# AudioDown 1.0 Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the first independently runnable AudioDown 1.0 slice: a Rust Core, a restricted Rust Supervisor, SQLite-backed system state, structured logs, an embedded Vue empty-state UI, and a virtual Node.js plugin that can be started on demand and complete a JSON-RPC handshake.

**Architecture:** The public Core container serves Axum HTTP APIs and the embedded Vue UI, owns SQLite and structured logs, and talks to Supervisor only through a shared Unix socket. Supervisor alone receives the Docker Socket and may manage containers labeled for the current AudioDown installation. A virtual Node.js plugin runs without public network access and proves the manifest, lifecycle, health, RPC, and log boundaries before real platform code is introduced.

**Tech Stack:** Rust workspace, Axum, Tokio, SQLx/SQLite, tracing, serde, bollard, JSON-RPC 2.0, Unix sockets, Vue 3, TypeScript, Vite, Vitest, Docker Compose, Node.js 22.

---

## Delivery Roadmap

The approved design is intentionally split into six implementation plans. Each plan must leave the repository runnable and testable.

1. **Foundation and virtual plugin loop — this plan**
   - Workspace, Core, Supervisor, SQLite, logs, embedded UI, Compose, virtual plugin handshake.
2. **GitHub repository installation and secure Node builds**
   - Repository index, manifest validation, snapshot download, lockfile policy, fixed Dockerfile, risk grants.
3. **Content capabilities and discovery/search aggregation**
   - Capability router, search, discover, album, tracks, cursors, priority, fallback, empty states.
4. **Credential vault and credential plugin flows**
   - AES-256-GCM vault, temporary Cookie Jars, QR flows, manual import, scoped proxy injection.
5. **Task engine and core downloader**
   - Download plans, opaque resource refs, per-item resolution, retries, progress, pause/resume/cancel.
6. **Hardening, migration interfaces, diagnostics, and release**
   - Security test suite, old-data claim interfaces, diagnostic exports, multi-arch images, public docs.

This first plan deliberately does **not** implement GitHub installation, real HTTP proxying, credentials, real content data, or file downloads. It establishes the contracts and process boundaries those later plans depend on.

## Locked File Structure

```text
.
├── Cargo.toml                         Rust workspace members and shared dependency versions
├── Cargo.lock                         Reproducible Rust dependency graph
├── rust-toolchain.toml                Pinned Rust toolchain
├── crates/
│   ├── audiodown-domain/              Dependency-light domain and error types
│   ├── audiodown-plugin-api/          Manifest and JSON-RPC wire contracts
│   ├── audiodown-storage/             SQLx pool, migrations, system/plugin/log repositories
│   ├── audiodown-logging/             tracing setup and redaction helpers
│   ├── audiodown-server/              Axum Core binary, APIs, embedded UI, Supervisor client
│   └── audiodown-supervisor/          Restricted Docker-management binary and Unix RPC server
├── migrations/                        SQLite schema owned by audiodown-storage
├── web/                               Vue application and generated dist assets
├── plugin-sdk/node/                   Minimal Node SDK for protocol handshake and structured logs
├── test-fixtures/plugins/virtual/     Virtual plugin manifest and Node implementation
├── docker/
│   ├── core.Dockerfile                Multi-stage Core + Vue production image
│   ├── supervisor.Dockerfile          Supervisor production image
│   └── plugin-runtime/node22.Dockerfile Fixed Node plugin runtime image
├── docker-compose.yml                 User-facing two-container deployment
├── tests/                             Shell-based Docker integration tests
└── docs/superpowers/plans/            Phase plans and roadmap
```

No file in this phase may contain a real platform name, platform domain, authorization-code logic, or real Cookie handling.

### Task 1: Bootstrap the Rust Workspace

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `crates/audiodown-domain/Cargo.toml`
- Create: `crates/audiodown-domain/src/lib.rs`
- Create: `crates/audiodown-plugin-api/Cargo.toml`
- Create: `crates/audiodown-plugin-api/src/lib.rs`
- Create: `crates/audiodown-storage/Cargo.toml`
- Create: `crates/audiodown-storage/src/lib.rs`
- Create: `crates/audiodown-logging/Cargo.toml`
- Create: `crates/audiodown-logging/src/lib.rs`
- Create: `crates/audiodown-server/Cargo.toml`
- Create: `crates/audiodown-server/src/main.rs`
- Create: `crates/audiodown-supervisor/Cargo.toml`
- Create: `crates/audiodown-supervisor/src/main.rs`
- Modify: `.gitignore`

- [ ] **Step 1: Add a Docker-based workspace smoke test that fails before the workspace exists**

Create `tests/workspace-smoke.sh`:

```sh
#!/bin/sh
set -eu

docker run --rm \
  -v "$(pwd):/workspace" \
  -w /workspace \
  rust:1.88-bookworm \
  cargo metadata --no-deps --format-version 1 >/tmp/audiodown-metadata.json

grep -q 'audiodown-server' /tmp/audiodown-metadata.json
grep -q 'audiodown-supervisor' /tmp/audiodown-metadata.json
```

Make it executable:

```bash
chmod +x tests/workspace-smoke.sh
```

- [ ] **Step 2: Run the smoke test and verify it fails**

Run:

```bash
./tests/workspace-smoke.sh
```

Expected: FAIL because `Cargo.toml` does not exist.

- [ ] **Step 3: Create the workspace manifest and pinned toolchain**

Create `Cargo.toml`:

```toml
[workspace]
resolver = "2"
members = [
  "crates/audiodown-domain",
  "crates/audiodown-plugin-api",
  "crates/audiodown-storage",
  "crates/audiodown-logging",
  "crates/audiodown-server",
  "crates/audiodown-supervisor",
]

[workspace.package]
version = "1.0.0-alpha.1"
edition = "2021"
license = "Apache-2.0"
rust-version = "1.88"

[workspace.dependencies]
anyhow = "1.0"
async-trait = "0.1"
axum = "0.8"
base64 = "0.22"
bollard = "0.19"
chrono = { version = "0.4", features = ["serde"] }
futures-util = "0.3"
http = "1.3"
regex = "1.11"
semver = { version = "1.0", features = ["serde"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
sha2 = "0.10"
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio-rustls", "sqlite", "migrate", "chrono", "uuid"] }
thiserror = "2.0"
tokio = { version = "1.46", features = ["macros", "rt-multi-thread", "net", "signal", "time", "fs", "sync", "process", "io-util"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "request-id", "trace", "fs"] }
tracing = "0.1"
tracing-appender = "0.2"
tracing-subscriber = { version = "0.3", features = ["env-filter", "fmt", "json"] }
url = { version = "2.5", features = ["serde"] }
uuid = { version = "1.17", features = ["v4", "serde"] }

[profile.release]
lto = true
codegen-units = 1
strip = "symbols"
panic = "abort"
```

Create `rust-toolchain.toml`:

```toml
[toolchain]
channel = "1.88.0"
profile = "minimal"
components = ["clippy", "rustfmt"]
```

- [ ] **Step 4: Create minimal crate manifests and compilable placeholders**

Use this library manifest pattern for domain, plugin API, storage, and logging, changing only `name`:

```toml
[package]
name = "audiodown-domain"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
serde.workspace = true
```

Create each library `src/lib.rs` with:

```rust
#![forbid(unsafe_code)]

pub const CRATE_READY: bool = true;
```

Create `crates/audiodown-server/Cargo.toml`:

```toml
[package]
name = "audiodown-server"
version.workspace = true
edition.workspace = true
license.workspace = true
rust-version.workspace = true

[dependencies]
anyhow.workspace = true
tokio.workspace = true
```

Create `crates/audiodown-server/src/main.rs`:

```rust
#![forbid(unsafe_code)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("audiodown-server bootstrap");
    Ok(())
}
```

Create `crates/audiodown-supervisor/Cargo.toml` with the same dependencies and create `src/main.rs`:

```rust
#![forbid(unsafe_code)]

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("audiodown-supervisor bootstrap");
    Ok(())
}
```

Append to `.gitignore`:

```gitignore
/target/
**/*.db
**/*.db-shm
**/*.db-wal
/data/
```

- [ ] **Step 5: Format and run the workspace smoke test**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm cargo fmt --all -- --check
./tests/workspace-smoke.sh
```

Expected: both commands PASS and `Cargo.lock` is generated.

- [ ] **Step 6: Commit the workspace bootstrap**

```bash
git add Cargo.toml Cargo.lock rust-toolchain.toml crates .gitignore tests/workspace-smoke.sh
git commit -m "build: bootstrap AudioDown Rust workspace"
```

### Task 2: Define Stable Domain and Plugin Wire Types

**Files:**
- Modify: `crates/audiodown-domain/Cargo.toml`
- Replace: `crates/audiodown-domain/src/lib.rs`
- Create: `crates/audiodown-domain/src/plugin.rs`
- Create: `crates/audiodown-domain/src/log.rs`
- Modify: `crates/audiodown-plugin-api/Cargo.toml`
- Replace: `crates/audiodown-plugin-api/src/lib.rs`
- Create: `crates/audiodown-plugin-api/src/manifest.rs`
- Create: `crates/audiodown-plugin-api/src/rpc.rs`
- Test: `crates/audiodown-plugin-api/tests/contracts.rs`

- [ ] **Step 1: Write failing contract tests**

Create `crates/audiodown-plugin-api/tests/contracts.rs`:

```rust
use audiodown_plugin_api::{
    manifest::{PluginManifest, PluginType, RuntimeKind},
    rpc::{JsonRpcRequest, PluginHello, PROTOCOL_VERSION},
};

#[test]
fn parses_minimal_node_content_manifest() {
    let manifest: PluginManifest = serde_json::from_str(r#"{
      "schemaVersion":"1.0",
      "id":"com.example.virtual.content",
      "name":"Virtual Content",
      "version":"1.0.0",
      "type":"content",
      "runtime":{"type":"nodejs","version":"22","entry":"src/index.js"},
      "compatibility":{"pluginApi":">=1.0 <2.0","core":">=1.0 <2.0"},
      "platform":{"id":"virtual","name":"Virtual"},
      "capabilities":["system.health"],
      "network":{"allowedHosts":[]}
    }"#).unwrap();

    assert_eq!(manifest.plugin_type, PluginType::Content);
    assert_eq!(manifest.runtime.kind, RuntimeKind::Nodejs);
    assert_eq!(manifest.id.as_str(), "com.example.virtual.content");
}

#[test]
fn serializes_json_rpc_hello_request() {
    let request = JsonRpcRequest::new(
        "req-1",
        "system.hello",
        PluginHello {
            protocol_version: PROTOCOL_VERSION.to_string(),
            core_version: "1.0.0-alpha.1".to_string(),
        },
    ).unwrap();

    let json = serde_json::to_value(request).unwrap();
    assert_eq!(json["jsonrpc"], "2.0");
    assert_eq!(json["method"], "system.hello");
    assert_eq!(json["params"]["protocolVersion"], "1.0");
}
```

- [ ] **Step 2: Run the tests and verify they fail**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-api --test contracts
```

Expected: FAIL because manifest and RPC modules do not exist.

- [ ] **Step 3: Implement domain identifiers and structured log types**

Set `crates/audiodown-domain/Cargo.toml` dependencies:

```toml
[dependencies]
chrono.workspace = true
regex.workspace = true
serde.workspace = true
thiserror.workspace = true
uuid.workspace = true
```

Create `crates/audiodown-domain/src/plugin.rs` with a validated `PluginId`, `PluginStatus`, and `RunMode`. `PluginId::parse` must accept only lowercase ASCII letters, digits, `.`, `_`, and `-`, must reject leading/trailing punctuation, and must cap length at 128 characters.

Use these public signatures:

```rust
pub struct PluginId(String);
impl PluginId {
    pub fn parse(input: impl Into<String>) -> Result<Self, PluginIdError>;
    pub fn as_str(&self) -> &str;
}

pub enum PluginStatus { Installed, Starting, Healthy, Stopped, Unhealthy, Disabled }
pub enum RunMode { OnDemand, Always }
```

Create `crates/audiodown-domain/src/log.rs`:

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel { Trace, Debug, Info, Warn, Error }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredLog {
    pub id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub component: String,
    pub message: String,
    pub plugin_id: Option<String>,
    pub plugin_version: Option<String>,
    pub platform_id: Option<String>,
    pub request_id: Option<String>,
    pub task_id: Option<String>,
    pub container_id: Option<String>,
    pub error_code: Option<String>,
    pub context: serde_json::Value,
}
```

Export modules from `lib.rs`:

```rust
#![forbid(unsafe_code)]

pub mod log;
pub mod plugin;
```

- [ ] **Step 4: Implement manifest and JSON-RPC wire contracts**

Set `crates/audiodown-plugin-api/Cargo.toml` dependencies:

```toml
[dependencies]
audiodown-domain = { path = "../audiodown-domain" }
semver.workspace = true
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
```

Create manifest types with exact serialized names used by the test:

```rust
pub struct PluginManifest {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    pub id: PluginId,
    pub name: String,
    pub version: Version,
    #[serde(rename = "type")]
    pub plugin_type: PluginType,
    pub runtime: RuntimeSpec,
    pub compatibility: CompatibilitySpec,
    pub platform: PlatformSpec,
    pub capabilities: Vec<String>,
    pub network: NetworkPolicy,
}
```

Define `PluginType::{Content, Credential}`, `RuntimeKind::Nodejs`, runtime version/entry, compatibility ranges as strings for phase one, platform id/name, and `NetworkPolicy { allowed_hosts: Vec<String> }`.

Create `rpc.rs` with:

```rust
pub const PROTOCOL_VERSION: &str = "1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: String,
    pub method: String,
    pub params: serde_json::Value,
}

impl JsonRpcRequest {
    pub fn new<T: Serialize>(id: impl Into<String>, method: impl Into<String>, params: T)
        -> Result<Self, serde_json::Error>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginHello {
    pub protocol_version: String,
    pub core_version: String,
}
```

Also define `JsonRpcResponse`, `JsonRpcError`, `PluginHelloResult`, and `PluginHealthResult` for later tasks.

- [ ] **Step 5: Run contract tests and clippy**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-api --test contracts && cargo clippy --workspace --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 6: Commit the contracts**

```bash
git add crates/audiodown-domain crates/audiodown-plugin-api
git commit -m "feat: define plugin protocol contracts"
```

### Task 3: Add SQLite as the Single Source of Truth

**Files:**
- Create: `migrations/0001_initial.sql`
- Modify: `crates/audiodown-storage/Cargo.toml`
- Replace: `crates/audiodown-storage/src/lib.rs`
- Create: `crates/audiodown-storage/src/plugin_repository.rs`
- Create: `crates/audiodown-storage/src/log_repository.rs`
- Test: `crates/audiodown-storage/tests/storage.rs`

- [ ] **Step 1: Write failing repository tests**

Create tests that open `sqlite::memory:`, run migrations, insert one plugin record, update its status, append one structured log, and query logs by `plugin_id`. Use these assertions:

```rust
assert_eq!(plugin.status, PluginStatus::Healthy);
assert_eq!(logs.len(), 1);
assert_eq!(logs[0].message, "virtual plugin ready");
```

Use public APIs:

```rust
let storage = Storage::connect("sqlite::memory:").await?;
storage.migrate().await?;
storage.plugins().upsert(&record).await?;
storage.plugins().set_status(&plugin_id, PluginStatus::Healthy).await?;
storage.logs().append(&log).await?;
let logs = storage.logs().list(LogFilter { plugin_id: Some(plugin_id), limit: 50 }).await?;
```

- [ ] **Step 2: Run the storage test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-storage --test storage
```

Expected: FAIL because Storage and repositories are missing.

- [ ] **Step 3: Create the first migration**

Create `migrations/0001_initial.sql` with:

```sql
PRAGMA foreign_keys = ON;

CREATE TABLE system_meta (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE plugins (
  plugin_id TEXT PRIMARY KEY,
  plugin_type TEXT NOT NULL,
  platform_id TEXT NOT NULL,
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  protocol_version TEXT NOT NULL,
  source_kind TEXT NOT NULL,
  source_ref TEXT NOT NULL,
  commit_sha TEXT,
  manifest_json TEXT NOT NULL,
  manifest_hash TEXT NOT NULL,
  image_id TEXT,
  status TEXT NOT NULL,
  run_mode TEXT NOT NULL DEFAULT 'on_demand',
  priority INTEGER NOT NULL DEFAULT 100,
  enabled INTEGER NOT NULL DEFAULT 1,
  last_error TEXT,
  installed_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE structured_logs (
  id TEXT PRIMARY KEY,
  timestamp TEXT NOT NULL,
  level TEXT NOT NULL,
  component TEXT NOT NULL,
  message TEXT NOT NULL,
  plugin_id TEXT,
  plugin_version TEXT,
  platform_id TEXT,
  request_id TEXT,
  task_id TEXT,
  container_id TEXT,
  error_code TEXT,
  context_json TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX idx_structured_logs_timestamp ON structured_logs(timestamp DESC);
CREATE INDEX idx_structured_logs_plugin ON structured_logs(plugin_id, timestamp DESC);
CREATE INDEX idx_structured_logs_request ON structured_logs(request_id, timestamp DESC);
```

- [ ] **Step 4: Implement Storage and repositories**

Set storage dependencies:

```toml
[dependencies]
audiodown-domain = { path = "../audiodown-domain" }
audiodown-plugin-api = { path = "../audiodown-plugin-api" }
async-trait.workspace = true
chrono.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
thiserror.workspace = true
uuid.workspace = true
```

Implement:

```rust
pub struct Storage { pool: SqlitePool }
impl Storage {
    pub async fn connect(url: &str) -> Result<Self, StorageError>;
    pub async fn migrate(&self) -> Result<(), StorageError>;
    pub fn plugins(&self) -> PluginRepository<'_>;
    pub fn logs(&self) -> LogRepository<'_>;
}
```

`connect` must enable WAL for file databases, foreign keys, a 5-second busy timeout, and `max_connections(5)`. `migrate` must use `sqlx::migrate!("../../migrations")`.

Define `PluginRecord`, `LogFilter`, repository insert/update/list methods, and explicit string conversions for enums. Reject unknown database enum values with `StorageError::InvalidData` rather than silently defaulting.

- [ ] **Step 5: Run migration and repository tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-storage --test storage && cargo clippy -p audiodown-storage --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 6: Commit storage**

```bash
git add migrations crates/audiodown-storage
git commit -m "feat: add SQLite system repositories"
```

### Task 4: Implement Structured Logging and Redaction

**Files:**
- Modify: `crates/audiodown-logging/Cargo.toml`
- Replace: `crates/audiodown-logging/src/lib.rs`
- Create: `crates/audiodown-logging/src/redaction.rs`
- Test: `crates/audiodown-logging/tests/redaction.rs`

- [ ] **Step 1: Write failing redaction tests**

Test that these inputs never survive output:

```rust
let input = r#"Cookie: session=secret123; Authorization: Bearer token456; phone=13800138000"#;
let output = redact_text(input);
assert!(!output.contains("secret123"));
assert!(!output.contains("token456"));
assert!(!output.contains("13800138000"));
assert!(output.contains("[REDACTED]"));
```

Also test recursive JSON redaction for keys matching `cookie`, `authorization`, `token`, `password`, `secret`, and `set-cookie`, case-insensitively.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-logging --test redaction
```

Expected: FAIL because redaction helpers are missing.

- [ ] **Step 3: Implement redaction and tracing initialization**

Use exact public functions:

```rust
pub fn redact_text(input: &str) -> String;
pub fn redact_json(value: &serde_json::Value) -> serde_json::Value;
pub fn init_logging(log_dir: &Path, filter: &str) -> anyhow::Result<LoggingGuard>;
```

`redact_text` must redact header-style Cookie/Authorization values, bearer tokens, 11-digit mainland mobile numbers, and URL query keys named token/access_token/password/secret. `redact_json` must recursively replace sensitive values with `"[REDACTED]"`.

`init_logging` must write JSON lines to a daily rolling file under `/data/logs` and human-readable logs to stdout. Return a guard that keeps the non-blocking file writer alive.

- [ ] **Step 4: Run tests and clippy**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-logging && cargo clippy -p audiodown-logging --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 5: Commit logging**

```bash
git add crates/audiodown-logging
git commit -m "feat: add structured logging redaction"
```

### Task 5: Build the Core Health, System, Plugin, and Log APIs

**Files:**
- Modify: `crates/audiodown-server/Cargo.toml`
- Replace: `crates/audiodown-server/src/main.rs`
- Create: `crates/audiodown-server/src/app.rs`
- Create: `crates/audiodown-server/src/config.rs`
- Create: `crates/audiodown-server/src/state.rs`
- Create: `crates/audiodown-server/src/routes/health.rs`
- Create: `crates/audiodown-server/src/routes/system.rs`
- Create: `crates/audiodown-server/src/routes/plugins.rs`
- Create: `crates/audiodown-server/src/routes/logs.rs`
- Create: `crates/audiodown-server/src/routes/mod.rs`
- Test: `crates/audiodown-server/tests/http_api.rs`

- [ ] **Step 1: Write failing Axum API tests**

Create an in-process app with in-memory SQLite and assert:

```text
GET /healthz -> 200 {"ok":true,"service":"audiodown-core"}
GET /api/v1/system -> 200 with version, supervisor status, plugin count
GET /api/v1/plugins -> 200 {"items":[]}
GET /api/v1/logs -> 200 {"items":[]}
GET /api/v1/discover -> 200 empty-state payload
GET /api/v1/search?q=test -> 200 empty-state payload
```

The empty-state payload must contain `reason: "NO_CONTENT_PLUGINS"` and must not contain a real platform name.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-server --test http_api
```

Expected: FAIL because the router does not exist.

- [ ] **Step 3: Implement config, state, and router**

Use environment-backed config with these defaults:

```text
AUDIODOWN_BIND=0.0.0.0:18080
AUDIODOWN_DATA_DIR=/data
AUDIODOWN_DATABASE_URL=sqlite:///data/audiodown.db
AUDIODOWN_SUPERVISOR_SOCKET=/run/audiodown/supervisor.sock
AUDIODOWN_LOG=info
```

`AppState` must contain `Storage`, semantic core version, and a `SupervisorClient` trait object. For this task, provide `UnavailableSupervisorClient` that reports unavailable without touching Docker.

Implement `build_router(state)` and use typed JSON response structs. Do not return raw `serde_json::Value` from route handlers except for structured log context.

- [ ] **Step 4: Implement startup and graceful shutdown**

`main.rs` must:

1. Load config.
2. Create `/data`, `/data/logs`, and `/data/plugins`.
3. Initialize logging.
4. Connect SQLite and run migrations.
5. Build the Axum router.
6. Bind TCP.
7. Handle SIGINT/SIGTERM graceful shutdown.

- [ ] **Step 5: Run API tests and a containerized server smoke test**

Run tests:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-server --test http_api && cargo clippy -p audiodown-server --all-targets -- -D warnings'
```

Then run the binary in Docker:

```bash
docker run --rm -d --name audiodown-core-smoke \
  -p 18081:18080 \
  -v "$(pwd):/workspace" \
  -w /workspace \
  rust:1.88-bookworm \
  sh -lc 'cargo run -p audiodown-server'
sleep 3
curl --fail http://127.0.0.1:18081/healthz
docker rm -f audiodown-core-smoke
```

Expected: health response contains `"ok":true`.

- [ ] **Step 6: Commit Core APIs**

```bash
git add crates/audiodown-server
git commit -m "feat: add AudioDown Core HTTP APIs"
```

### Task 6: Implement the Restricted Supervisor Protocol

**Files:**
- Modify: `crates/audiodown-supervisor/Cargo.toml`
- Replace: `crates/audiodown-supervisor/src/main.rs`
- Create: `crates/audiodown-supervisor/src/config.rs`
- Create: `crates/audiodown-supervisor/src/protocol.rs`
- Create: `crates/audiodown-supervisor/src/policy.rs`
- Create: `crates/audiodown-supervisor/src/docker.rs`
- Create: `crates/audiodown-supervisor/src/server.rs`
- Test: `crates/audiodown-supervisor/tests/policy.rs`
- Test: `crates/audiodown-supervisor/tests/protocol.rs`

- [ ] **Step 1: Write failing policy tests**

Test that a generated plugin container specification always enforces:

```rust
assert!(spec.read_only);
assert_eq!(spec.memory_bytes, 128 * 1024 * 1024);
assert_eq!(spec.nano_cpus, 500_000_000);
assert_eq!(spec.pids_limit, 64);
assert!(spec.cap_drop.contains(&"ALL".to_string()));
assert!(!spec.mounts.iter().any(|m| m.contains("docker.sock")));
assert!(!spec.host_network);
assert_eq!(spec.labels["io.audiodown.managed"], "true");
```

Also test that `PluginId` is the only caller-supplied lifecycle parameter and that arbitrary image names, commands, mounts, environment variables, and container names cannot be deserialized into requests.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-supervisor
```

Expected: FAIL because policy and protocol modules are missing.

- [ ] **Step 3: Define the Supervisor request protocol**

Allow only these phase-one methods:

```text
system.ping
plugin.start
plugin.stop
plugin.inspect
plugin.logs
```

Request parameters for lifecycle methods contain only:

```rust
pub struct PluginRequest { pub plugin_id: PluginId }
```

`plugin.start` loads image ID, manifest, limits, installation ID, and runtime path from Supervisor-owned `/data/plugins/installed/<plugin-id>/install.json`. Core cannot override them.

- [ ] **Step 4: Implement the container policy builder**

Create a pure `PluginContainerPolicy::build(InstalledPlugin)` function. It must generate deterministic container name `audiodown-plugin-<sha256(plugin_id)[0..12]>`, required labels, read-only root, tmpfs `/tmp:rw,noexec,nosuid,nodev,size=67108864`, no public ports, no privileged mode, no added capabilities, dropped `ALL`, `no-new-privileges`, PID/memory/CPU limits, and only the plugin RPC socket mount.

Keep policy creation separate from bollard calls so all security invariants are unit-testable.

- [ ] **Step 5: Implement Unix Socket server and Docker adapter**

Supervisor defaults:

```text
AUDIODOWN_SUPERVISOR_SOCKET=/run/audiodown/supervisor.sock
AUDIODOWN_PLUGIN_DATA=/data/plugins
AUDIODOWN_INSTALLATION_ID_FILE=/data/plugins/installation-id
AUDIODOWN_CORE_TOKEN_FILE=/run/audiodown/core.token
```

On first start, create installation ID and token with mode `0600`. Each request must include token, timestamp, and nonce. Reject timestamps outside ±30 seconds and duplicate nonces retained for 2 minutes.

Use newline-delimited JSON over Unix Socket for the Supervisor protocol. Limit each request to 64 KiB and close the connection after one response.

The Docker adapter uses bollard, filters all operations by installation and managed labels, and refuses a found container whose labels do not match the requested plugin ID.

- [ ] **Step 6: Run unit tests and a Docker-backed ping test**

Run unit tests:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-supervisor && cargo clippy -p audiodown-supervisor --all-targets -- -D warnings'
```

Run Supervisor with only a temporary socket and verify `system.ping`; do not exercise plugin start until Task 9 installs a fixture.

Expected ping result:

```json
{"ok":true,"service":"audiodown-supervisor"}
```

- [ ] **Step 7: Commit Supervisor protocol and policy**

```bash
git add crates/audiodown-supervisor
git commit -m "feat: add restricted plugin supervisor"
```

### Task 7: Connect Core to Supervisor

**Files:**
- Create: `crates/audiodown-server/src/supervisor.rs`
- Modify: `crates/audiodown-server/src/state.rs`
- Modify: `crates/audiodown-server/src/routes/system.rs`
- Modify: `crates/audiodown-server/src/routes/plugins.rs`
- Modify: `crates/audiodown-server/src/main.rs`
- Test: `crates/audiodown-server/tests/supervisor_client.rs`

- [ ] **Step 1: Write failing client tests against a temporary Unix server**

Test successful ping, malformed response, timeout, missing socket, and oversized response. Use a 2-second default timeout and a 1 MiB maximum response.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-server --test supervisor_client
```

Expected: FAIL because `UnixSupervisorClient` is missing.

- [ ] **Step 3: Implement the typed Supervisor client**

Define trait:

```rust
#[async_trait]
pub trait SupervisorClient: Send + Sync {
    async fn ping(&self) -> Result<SupervisorHealth, SupervisorError>;
    async fn start_plugin(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, SupervisorError>;
    async fn stop_plugin(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, SupervisorError>;
    async fn inspect_plugin(&self, plugin_id: &PluginId) -> Result<PluginRuntimeState, SupervisorError>;
}
```

The Unix implementation reads the token file, generates nonce/timestamp, sends one newline-delimited request, and maps protocol errors to stable Core error codes.

- [ ] **Step 4: Wire live Supervisor status into Core APIs**

`GET /api/v1/system` reports `supervisor.available` and an error summary. Add:

```text
POST /api/v1/plugins/:plugin_id/start
POST /api/v1/plugins/:plugin_id/stop
```

Return HTTP 503 with `SUPERVISOR_UNAVAILABLE` when the socket cannot be reached. Do not expose the token, socket path, Docker errors, or container internals.

- [ ] **Step 5: Run Core and Supervisor tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-server && cargo test -p audiodown-supervisor'
```

Expected: PASS.

- [ ] **Step 6: Commit Core/Supervisor integration**

```bash
git add crates/audiodown-server
git commit -m "feat: connect Core to Supervisor"
```

### Task 8: Build the Vue Empty-State UI

**Files:**
- Create: `web/package.json`
- Create: `web/package-lock.json`
- Create: `web/tsconfig.json`
- Create: `web/vite.config.ts`
- Create: `web/index.html`
- Create: `web/src/main.ts`
- Create: `web/src/App.vue`
- Create: `web/src/api/client.ts`
- Create: `web/src/router.ts`
- Create: `web/src/styles.css`
- Create: `web/src/views/DiscoverView.vue`
- Create: `web/src/views/SearchView.vue`
- Create: `web/src/views/PluginsView.vue`
- Create: `web/src/views/LogsView.vue`
- Create: `web/src/views/SystemView.vue`
- Test: `web/src/views/empty-state.test.ts`
- Modify: `.gitignore`

- [ ] **Step 1: Write failing UI tests**

Using Vitest and Vue Test Utils, assert that Discover and Search render:

```text
尚未安装内容插件
添加 GitHub 插件仓库
```

Assert Plugins shows Supervisor availability and no hardcoded platform labels.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
docker run --rm -v "$(pwd)/web:/app" -w /app node:22-bookworm-slim \
  sh -lc 'npm ci && npm test -- --run'
```

Expected: FAIL because the Vue app is missing.

- [ ] **Step 3: Create the minimal Vue application**

Use dependencies:

```json
{
  "dependencies": {
    "@vitejs/plugin-vue": "latest",
    "vue": "latest",
    "vue-router": "latest"
  },
  "devDependencies": {
    "@types/node": "latest",
    "@vue/test-utils": "latest",
    "jsdom": "latest",
    "typescript": "latest",
    "vite": "latest",
    "vitest": "latest"
  }
}
```

After the initial `npm install`, replace `latest` ranges with the exact resolved versions written to `package-lock.json` so future installs are reproducible.

Create a simple left navigation with Discover, Search, Plugins, Logs, and System. Do not add authentication, themes, plugin forms, or real platform imagery in phase one.

The API client must call relative `/api/v1/...` paths and provide typed response interfaces. Empty-state pages consume Core responses rather than hardcoding whether plugins exist.

- [ ] **Step 4: Run UI tests, typecheck, and production build**

Run:

```bash
docker run --rm -v "$(pwd)/web:/app" -w /app node:22-bookworm-slim \
  sh -lc 'npm ci && npm test -- --run && npm run typecheck && npm run build'
```

Expected: PASS and `web/dist/index.html` exists.

- [ ] **Step 5: Update ignore rules and commit UI**

Append:

```gitignore
/web/node_modules/
/web/dist/
```

Commit:

```bash
git add web .gitignore
git commit -m "feat: add plugin-first empty-state UI"
```

### Task 9: Embed the Vue Build in Core

**Files:**
- Modify: `crates/audiodown-server/Cargo.toml`
- Create: `crates/audiodown-server/build.rs`
- Create: `crates/audiodown-server/src/web.rs`
- Modify: `crates/audiodown-server/src/app.rs`
- Test: `crates/audiodown-server/tests/web_assets.rs`

- [ ] **Step 1: Write failing asset tests**

Assert `GET /` returns HTML containing `AudioDown 1.0`, unknown non-API routes return the SPA index, and `/api/v1/not-found` remains JSON 404 rather than SPA HTML.

- [ ] **Step 2: Run tests and verify failure**

Run the web build, then:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-server --test web_assets
```

Expected: FAIL because assets are not embedded.

- [ ] **Step 3: Add deterministic asset embedding**

Use `rust-embed` and MIME detection. `build.rs` must fail with a clear message if `web/dist/index.html` is missing:

```text
web/dist is missing; run npm ci && npm run build in web/
```

Serve hashed assets with one-year immutable cache headers and `index.html` with `no-cache`.

- [ ] **Step 4: Run API and asset tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-server --test web_assets && cargo test -p audiodown-server --test http_api'
```

Expected: PASS.

- [ ] **Step 5: Commit embedded UI**

```bash
git add crates/audiodown-server
git commit -m "feat: embed Vue UI in Core"
```

### Task 10: Create the Node Plugin SDK and Virtual Plugin

**Files:**
- Create: `plugin-sdk/node/package.json`
- Create: `plugin-sdk/node/package-lock.json`
- Create: `plugin-sdk/node/src/index.js`
- Create: `plugin-sdk/node/src/rpc.js`
- Create: `plugin-sdk/node/src/logger.js`
- Test: `plugin-sdk/node/test/sdk.test.js`
- Create: `test-fixtures/plugins/virtual/audiodown-plugin.json`
- Create: `test-fixtures/plugins/virtual/package.json`
- Create: `test-fixtures/plugins/virtual/package-lock.json`
- Create: `test-fixtures/plugins/virtual/src/index.js`

- [ ] **Step 1: Write failing Node SDK tests**

Test newline-delimited JSON parsing, JSON-RPC response generation, a 1 MiB message limit, protocol hello response, health response, and structured log output.

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
docker run --rm -v "$(pwd)/plugin-sdk/node:/app" -w /app node:22-bookworm-slim \
  sh -lc 'npm ci && npm test'
```

Expected: FAIL because SDK modules are missing.

- [ ] **Step 3: Implement dependency-free SDK**

The SDK must use Node built-ins only. Export:

```javascript
createPluginServer({ manifest, handlers, input, output })
createLogger({ output })
RpcError
```

Built-in handlers:

```text
system.hello
system.health
system.shutdown
```

The server processes one JSON object per line, serializes one response per line, never logs to protocol stdout, and sends logs through a dedicated `log.emit` JSON-RPC notification stream on stderr for phase one.

- [ ] **Step 4: Implement the virtual plugin fixture**

Manifest ID: `com.audiodown.virtual.content`.

Capabilities in phase one:

```json
["system.health"]
```

Allowed hosts: empty array. No real platform name or domain. The plugin must return its manifest ID, version, protocol version, and uptime from hello/health.

- [ ] **Step 5: Run SDK and fixture smoke tests**

Run:

```bash
docker run --rm -i \
  -v "$(pwd)/plugin-sdk/node:/sdk:ro" \
  -v "$(pwd)/test-fixtures/plugins/virtual:/plugin:ro" \
  -w /plugin node:22-bookworm-slim \
  node src/index.js <<'JSON'
{"jsonrpc":"2.0","id":"1","method":"system.health","params":{}}
JSON
```

Expected: one valid JSON-RPC response with `healthy: true`.

- [ ] **Step 6: Commit SDK and fixture**

```bash
git add plugin-sdk test-fixtures
git commit -m "feat: add Node plugin SDK and virtual plugin"
```

### Task 11: Add Fixed Production Dockerfiles and Extreme-Minimal Compose

**Files:**
- Create: `docker/core.Dockerfile`
- Create: `docker/supervisor.Dockerfile`
- Create: `docker/plugin-runtime/node22.Dockerfile`
- Create: `docker-compose.yml`
- Create: `.dockerignore`
- Create: `tests/compose-smoke.sh`

- [ ] **Step 1: Write the failing Compose smoke test**

The script must:

1. Run `docker compose config`.
2. Assert Docker Socket appears only under Supervisor.
3. Build Core and Supervisor.
4. Start the stack.
5. Wait up to 60 seconds for `/healthz`.
6. Assert `/api/v1/system` reports Supervisor available.
7. Assert `/api/v1/plugins` initially returns an empty array.
8. Tear down the stack without deleting `./data`.

- [ ] **Step 2: Run the smoke test and verify failure**

Run:

```bash
./tests/compose-smoke.sh
```

Expected: FAIL because Dockerfiles and Compose do not exist.

- [ ] **Step 3: Create production Dockerfiles**

`core.Dockerfile` stages:

1. Node 22 builds `web/dist` with `npm ci`.
2. Rust 1.88 builds `audiodown-server --release`.
3. Debian bookworm-slim runtime includes CA certificates, runs as non-root UID 10001, exposes 18080, and uses `/data`.

`supervisor.Dockerfile` stages:

1. Rust 1.88 builds `audiodown-supervisor --release`.
2. Debian bookworm-slim runtime includes CA certificates and runs with only the permissions needed to access the mounted Docker Socket.

`plugin-runtime/node22.Dockerfile` must use `node:22-bookworm-slim`, create non-root UID 10002, copy a fixed runner, set read-only-compatible paths, and define no platform-specific code.

- [ ] **Step 4: Create the user-facing Compose file**

Use exactly two services and one named control volume:

```yaml
services:
  audiodown:
    build:
      context: .
      dockerfile: docker/core.Dockerfile
    image: audiodown/core:1.0.0-alpha.1
    container_name: audiodown
    restart: unless-stopped
    ports:
      - "${AUDIODOWN_PORT:-18080}:18080"
    volumes:
      - ./data:/data
      - audiodown-control:/run/audiodown

  supervisor:
    build:
      context: .
      dockerfile: docker/supervisor.Dockerfile
    image: audiodown/supervisor:1.0.0-alpha.1
    container_name: audiodown-supervisor
    restart: unless-stopped
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - ./data/plugins:/data/plugins
      - audiodown-control:/run/audiodown

volumes:
  audiodown-control:
```

Do not expose Supervisor ports. Do not mount Docker Socket into Core.

- [ ] **Step 5: Run Compose smoke test**

Run:

```bash
chmod +x tests/compose-smoke.sh
./tests/compose-smoke.sh
```

Expected: PASS.

- [ ] **Step 6: Commit container packaging**

```bash
git add docker docker-compose.yml .dockerignore tests/compose-smoke.sh
git commit -m "build: add minimal two-container deployment"
```

### Task 12: Install and Start the Virtual Plugin Fixture

**Files:**
- Create: `crates/audiodown-supervisor/src/install_record.rs`
- Modify: `crates/audiodown-supervisor/src/docker.rs`
- Modify: `crates/audiodown-supervisor/src/server.rs`
- Create: `scripts/install-virtual-plugin.sh`
- Create: `tests/virtual-plugin-smoke.sh`
- Modify: `crates/audiodown-server/src/routes/plugins.rs`

- [ ] **Step 1: Write the failing end-to-end test**

The test starts Compose, installs the virtual fixture into Supervisor-owned storage, inserts the corresponding plugin record through a phase-one development API, calls start, waits for healthy state, calls inspect, verifies logs include the plugin ID, calls stop, and verifies stopped state.

Expected lifecycle:

```text
installed -> starting -> healthy -> stopped
```

- [ ] **Step 2: Run the test and verify failure**

Run:

```bash
./tests/virtual-plugin-smoke.sh
```

Expected: FAIL because fixture installation and Docker start are incomplete.

- [ ] **Step 3: Implement Supervisor-owned install records**

Install record path:

```text
/data/plugins/installed/com.audiodown.virtual.content/install.json
```

Record fields:

```json
{
  "pluginId":"com.audiodown.virtual.content",
  "imageId":"audiodown/plugin-virtual:dev",
  "manifestPath":"/data/plugins/installed/com.audiodown.virtual.content/audiodown-plugin.json",
  "installationId":"generated-installation-id",
  "memoryBytes":134217728,
  "nanoCpus":500000000,
  "pidsLimit":64,
  "runMode":"on_demand"
}
```

Supervisor must reject missing files, mismatched plugin IDs, non-local image references in this phase, and manifest hashes that do not match the install record.

- [ ] **Step 4: Implement the development fixture installer**

`scripts/install-virtual-plugin.sh` must:

1. Build the fixed Node runtime plus virtual fixture into `audiodown/plugin-virtual:dev`.
2. Copy manifest into `data/plugins/installed/...`.
3. Compute SHA-256 manifest hash.
4. Preserve the installation ID generated by Supervisor.
5. Write install record atomically.
6. Call Core development endpoint `POST /api/v1/dev/plugins/register-fixture` enabled only when `AUDIODOWN_DEV_MODE=1`.

The development endpoint must reject non-loopback requests unless an explicit dev token is supplied. Production images default `AUDIODOWN_DEV_MODE=0`.

- [ ] **Step 5: Complete plugin container start and handshake**

On `plugin.start`, Supervisor creates/starts the container using the fixed policy, connects it only to the internal plugin network, waits for the RPC socket, performs `system.hello` and `system.health`, and returns healthy state. On handshake mismatch, stop the container and return `PLUGIN_NOT_COMPATIBLE`.

Capture plugin stderr and persist structured or fallback stdio logs through Core's internal log ingestion endpoint authenticated by the shared Core token.

- [ ] **Step 6: Run the end-to-end test**

Run:

```bash
chmod +x scripts/install-virtual-plugin.sh tests/virtual-plugin-smoke.sh
./tests/virtual-plugin-smoke.sh
```

Expected: PASS with the exact lifecycle and at least one log entry attributed to `com.audiodown.virtual.content`.

- [ ] **Step 7: Commit the first plugin lifecycle loop**

```bash
git add crates/audiodown-supervisor crates/audiodown-server scripts tests
git commit -m "feat: complete virtual plugin lifecycle"
```

### Task 13: Add Repository-Wide Verification and Security Assertions

**Files:**
- Create: `scripts/verify.sh`
- Create: `tests/security-boundary.sh`
- Create: `.github/workflows/ci.yml`
- Modify: `README.md`

- [ ] **Step 1: Write the security boundary script**

Assertions:

```text
Core container has no Docker Socket mount.
Supervisor has Docker Socket mount.
Virtual plugin has no public ports.
Virtual plugin is not privileged.
Virtual plugin has read-only rootfs.
Virtual plugin has CapDrop=ALL.
Virtual plugin has memory, CPU, and PID limits.
Virtual plugin cannot resolve or connect to a public test endpoint directly.
Virtual plugin cannot read /data/audiodown.db.
Virtual plugin cannot inspect /var/run/docker.sock.
```

Fail with a distinct message for every violated invariant.

- [ ] **Step 2: Run security script and fix any failures**

Run:

```bash
./tests/security-boundary.sh
```

Expected: PASS.

- [ ] **Step 3: Create the complete verification script**

`scripts/verify.sh` runs in order:

```text
Rust fmt
Rust workspace tests
Rust clippy -D warnings
Node SDK tests
Vue tests
Vue typecheck
Vue production build
Compose config
Compose smoke
Virtual plugin smoke
Security boundary checks
```

The script stops on first failure and always tears down test containers using a shell trap.

- [ ] **Step 4: Add CI**

GitHub Actions must run unit/static checks on every push and pull request. Docker integration jobs run on `ubuntu-latest`, use unique Compose project names, and upload Core/Supervisor/plugin logs when a test fails.

Do not publish images in this workflow.

- [ ] **Step 5: Update README with phase-one usage**

Document only verified commands:

```bash
docker compose up -d --build
curl http://localhost:18080/healthz
./scripts/install-virtual-plugin.sh
./scripts/verify.sh
```

Clearly state:

- No real platform plugin is included.
- No activation code is required.
- The virtual plugin exists only for contract testing.
- GitHub plugin installation, credentials, search data, and downloads arrive in later phases.

- [ ] **Step 6: Run full verification**

Run:

```bash
./scripts/verify.sh
```

Expected: all checks PASS.

- [ ] **Step 7: Commit verification and docs**

```bash
git add scripts tests .github README.md
git commit -m "test: verify AudioDown foundation boundaries"
```

### Task 14: Final Phase-One Acceptance Review

**Files:**
- Modify only files required to fix acceptance failures
- Create: `docs/phase-1-acceptance.md`

- [ ] **Step 1: Run the full verification from a clean checkout**

Clone into a temporary directory and run:

```bash
git clone . /tmp/audiodown-core-phase1-review
cd /tmp/audiodown-core-phase1-review
./scripts/verify.sh
```

Expected: PASS without relying on untracked files from the development checkout.

- [ ] **Step 2: Verify absence requirements**

Run repository scans that fail if they find:

```text
activation code models or routes
license heartbeat or device binding
real platform names or domains
Cookie parsing or storage implementations
GitHub automatic update behavior
archive/post-processing implementations
```

Allow these concepts only inside the approved design and plan documents where they describe exclusions or later phases.

- [ ] **Step 3: Record acceptance evidence**

Create `docs/phase-1-acceptance.md` containing:

- Commit SHA tested.
- Docker and Compose versions.
- Rust and Node image versions.
- Verification command and result.
- Core health response.
- Supervisor health response.
- Virtual plugin lifecycle evidence.
- Security boundary assertions.
- Known phase-one exclusions.

Do not include secrets, tokens, host paths, Docker Socket metadata, or complete environment dumps.

- [ ] **Step 4: Commit acceptance evidence**

```bash
git add docs/phase-1-acceptance.md
git commit -m "docs: record phase one acceptance"
```

- [ ] **Step 5: Push the completed phase branch**

```bash
git push origin HEAD
```

Expected: remote branch contains all phase-one commits and CI passes.

## Phase-One Definition of Done

The plan is complete only when all statements are true:

- `docker compose up -d --build` starts exactly Core and Supervisor as user-managed services.
- Core is the only service exposing port 18080.
- Docker Socket is mounted only into Supervisor.
- Core creates and migrates SQLite under `./data`.
- Core serves the embedded Vue UI and typed empty-state APIs.
- No real platform capability is present.
- No activation, license, device-binding, or heartbeat behavior exists.
- Supervisor enforces immutable plugin security policy.
- The virtual Node plugin can be installed, started on demand, handshaken, inspected, logged, and stopped.
- Plugin logs appear in the Core log API with plugin attribution and redaction.
- The plugin has no direct public network path, Core data mount, downloads mount, or Docker Socket.
- The complete verification script passes from a clean clone.

## Follow-Up Plan Boundaries

Do not add these while executing this plan:

- GitHub repository URL input or snapshot installation.
- npm dependency builds for untrusted repositories.
- Real search/discover data.
- Real credential storage or Cookie Jars.
- HTTP proxy requests to external hosts.
- Download tasks or file transfer.
- Plugin automatic update.
- Archive organization or post-processing.

Those belong to the subsequent plans listed in the roadmap and depend on the foundation verified here.
