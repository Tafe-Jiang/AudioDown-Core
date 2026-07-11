# AudioDown 1.0 Plugin Installation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add secure public GitHub repository inspection, immutable snapshot installation, fixed Node.js builds, plugin uninstall and runtime settings, without adding real content platforms, credentials, search data, downloads, or automatic updates.

**Architecture:** Core owns GitHub HTTP access, archive extraction, repository validation, staging metadata, SQLite business state, risk grants, and the public API/UI. The `audiodown-plugin-manager` service owns inspect/install/settings/uninstall use cases while Axum routes remain thin adapters. Supervisor remains the only Docker client: it revalidates Core-prepared operations, runs an asynchronous idempotent build state machine in a resource-limited temporary container, assembles a managed image from a pinned Node runtime, writes a runtime attestation, and removes managed resources. Core and Supervisor continue to communicate only through the authenticated Unix socket, and caller-controlled Docker fields never cross that protocol.

**Tech Stack:** Rust 1.88 workspace, Axum, Tokio, reqwest/rustls, serde, semver, tar/gzip, SHA-256/SHA-512 integrity checks, SQLx/SQLite, bollard, Docker internal networks, Node.js 22, npm lockfiles, Vue 3, TypeScript, Vitest, Docker Compose.

---

## Delivery Roadmap

The approved design remains split into independently testable plans:

1. Foundation and virtual plugin lifecycle - complete.
2. **GitHub repository installation and secure Node builds - this plan.**
3. Content capabilities and search/discovery aggregation.
4. Credential vault, credential plugins, and the runtime HTTP proxy.
5. Task engine and Core downloader.
6. Hardening, migration interfaces, diagnostics, and release.

This plan deliberately stops after plugin management. It does not invoke content capability RPC methods and does not implement real search, discover, album, track, credential, Cookie Jar, proxy, download, archive, migration, or update behavior.

## Locked Decisions

- Accept only public `https://github.com/{owner}/{repository}` URLs.
- Resolve the default branch once, lock the installation to a 40-character commit SHA, and never follow that branch automatically.
- Do not accept GitHub tokens, private repositories, arbitrary archive URLs, repository redirects, or update checks.
- Limit snapshots to 16 MiB compressed, 64 MiB extracted, 4 MiB per file, 2,048 files, and 32 plugins.
- Reject archive links, devices, FIFOs, path traversal, duplicate normalized paths, and files outside one top-level archive directory.
- Require `audiodown-repository.json`, `audiodown-plugin.json`, `package.json`, and `package-lock.json`.
- Support only Node.js 22 and plugin API/core compatibility within major version 1.
- Default builds run `npm ci --omit=dev --ignore-scripts`.
- Build-time network access goes through a Supervisor-managed proxy that accepts only `CONNECT registry.npmjs.org:443`; the untrusted build network has no direct egress.
- Supervisor allows one build globally and one operation per plugin. Each build is limited to 5 minutes, 512 MiB memory, 1 CPU, 128 PIDs, 256 MiB temporary output, and 1 MiB captured logs.
- The fixed Node 22 base image is locked by digest in the repository. Supervisor performs a controlled pull and verifies that digest before creating trusted builder/runtime images.
- Lifecycle scripts require an explicit manifest reason, Core developer mode, a valid development token, and a commit-specific risk grant. There is no general-purpose arbitrary build command.
- Runtime containers keep all phase-one restrictions: no public network, Core data, downloads, Docker Socket, added capabilities, privileged mode, or caller-controlled mounts.
- The public install endpoint waits synchronously in this phase. A failed build leaves no SQLite plugin row, managed image, prepared request, or partial installed directory.
- The public HTTP request may wait for installation, but Supervisor build RPC is asynchronous and idempotent. Core polls operation status with short RPC calls, commits SQLite before finalization, and reconciles interrupted operations after restart.
- SQLite is the only business source of truth. Supervisor operation files and `install.json` are runtime attestations that can be reconciled or rebuilt; they do not own enablement, priority, run mode, or user-facing state.
- Existing virtual fixture behavior remains available until the repository smoke test replaces its direct registration path.

## Execution Rules

- Start every Task with `git status --short --branch`; do not include unrelated work.
- Execute Tasks in order without merging or skipping them.
- For every Task: write the failing test, run it and observe the intended failure, implement only that Task, run the stated verification, then commit.
- Approved revision: Task 15 is expanded into the ten committed tasks in
  `2026-07-11-audiodown-mcp-ui-redesign-implementation-plan.md`; those commits
  replace the former single Task 15 commit.
- Use the suggested Chinese commit subject in the exact
  `阶段2：简要说明本次更新内容` format and add one concise Chinese commit body
  describing the change.
- Diagnose failing tests; never bypass, weaken, or delete them.
- Prefer `rust:1.88-bookworm` and `node:22-bookworm-slim` verification containers when local toolchains are unavailable.
- Do not push intermediate broken states. Push `main` only after the clean-clone verification passes.

## Locked File Structure

```text
crates/
├── audiodown-plugin-api/
│   └── src/
│       ├── manifest.rs                 Existing plugin manifest plus build-risk declaration
│       └── repository.rs               Repository index and preview wire contracts
├── audiodown-plugin-manager/
│   └── src/
│       ├── lib.rs                      Public manager interfaces and errors
│       ├── service.rs                  Inspect/install/settings/uninstall use cases
│       ├── github.rs                   Public GitHub URL parsing and immutable snapshot fetch
│       ├── archive.rs                  Bounded safe tar.gz extraction
│       ├── package.rs                  package.json/package-lock policy
│       ├── validation.rs               Index, manifest, compatibility, and source-tree checks
│       └── staging.rs                  Snapshot, risk-grant mirror, and prepared-operation files
├── audiodown-storage/
│   └── src/
│       ├── plugin_repository.rs        Plugin settings, deletion, and last-used state
│       └── risk_grant_repository.rs    Commit-specific lifecycle-script grants
├── audiodown-server/
│   └── src/
│       ├── lifecycle.rs                Always-run and idle-stop reconciliation
│       ├── plugin_manager_adapters.rs  SQLite and Supervisor manager ports
│       └── routes/
│           ├── repositories.rs         Repository inspection API
│           └── plugins.rs              Install, uninstall, settings, and lifecycle APIs
├── audiodown-supervisor-protocol/
│   └── src/lib.rs                      Shared Core/Supervisor request, operation, and result types
└── audiodown-supervisor/
    └── src/
        ├── build_proxy.rs              Fixed npm CONNECT allowlist proxy
        ├── builder.rs                  Restricted build container and managed image assembly
        ├── prepared_install.rs         Revalidation of Core-prepared operations
        └── install_record.rs           Runtime attestation validation
test-fixtures/
└── repositories/virtual/
    ├── audiodown-repository.json
    └── plugins/
        ├── virtual-content/            Dependency-free virtual Node plugin
        └── virtual-build-risk/         Observable lifecycle-script fixture
tests/
├── plugin-phase2-verification-wiring.sh Verification and CI wiring checks
├── plugin-repository-smoke.sh          Inspect, install, start, configure, and uninstall loop
└── plugin-installation-security.sh     Archive, build, protocol, and cleanup boundaries
```

### Task 1: Define Repository and Build-Risk Contracts

**Files:**
- Create: `crates/audiodown-plugin-api/src/repository.rs`
- Modify: `crates/audiodown-plugin-api/src/lib.rs`
- Modify: `crates/audiodown-plugin-api/src/manifest.rs`
- Test: `crates/audiodown-plugin-api/tests/repository_contracts.rs`

- [ ] **Step 1: Write failing repository contract tests**

Create tests that parse this minimal index:

```rust
use audiodown_plugin_api::{
    manifest::PluginManifest,
    repository::RepositoryIndex,
};

#[test]
fn parses_repository_index_and_declared_build_risk() {
    let index: RepositoryIndex = serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "repository": {
            "id": "example.plugins",
            "name": "Example Plugins"
        },
        "plugins": [
            {"path": "plugins/virtual-content"}
        ]
    }))
    .unwrap();
    assert_eq!(index.repository.id, "example.plugins");
    assert_eq!(index.plugins[0].path, "plugins/virtual-content");

    let manifest: PluginManifest = serde_json::from_value(serde_json::json!({
        "schemaVersion": "1.0",
        "id": "com.audiodown.virtual.content",
        "name": "Virtual Content",
        "version": "1.0.0",
        "type": "content",
        "runtime": {"type": "nodejs", "version": "22", "entry": "src/index.js"},
        "compatibility": {"pluginApi": ">=1.0 <2.0", "core": ">=1.0 <2.0"},
        "platform": {"id": "virtual", "name": "Virtual"},
        "capabilities": [],
        "network": {"allowedHosts": []},
        "build": {
            "npmLifecycleScripts": {
                "required": true,
                "reason": "Generate a deterministic local file"
            }
        }
    }))
    .unwrap();
    assert!(manifest.build.npm_lifecycle_scripts.required);
}

#[test]
fn manifest_defaults_to_no_lifecycle_scripts() {
    let manifest: PluginManifest =
        serde_json::from_str(include_str!("../../../test-fixtures/plugins/virtual/audiodown-plugin.json"))
            .unwrap();
    assert!(!manifest.build.npm_lifecycle_scripts.required);
}
```

- [ ] **Step 2: Run the contract test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-api --test repository_contracts
```

Expected: FAIL because `repository` and `build` contracts do not exist.

- [ ] **Step 3: Add the minimal wire types**

Add `repository.rs` with:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryIndex {
    pub schema_version: String,
    pub repository: RepositoryMetadata,
    pub plugins: Vec<RepositoryPluginRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryMetadata {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryPluginRef {
    pub path: String,
}
```

Extend `PluginManifest`:

```rust
#[serde(default)]
pub build: BuildSpec,

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuildSpec {
    #[serde(default)]
    pub npm_lifecycle_scripts: LifecycleScriptPolicy,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleScriptPolicy {
    #[serde(default)]
    pub required: bool,
    pub reason: Option<String>,
}
```

Export `pub mod repository;` from `lib.rs`.

- [ ] **Step 4: Run contract and existing plugin API tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-api && cargo clippy -p audiodown-plugin-api --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-api
git commit -m "阶段2：定义插件仓库契约" \
  -m "定义仓库索引、插件预览和构建风险的稳定契约。"
```

### Task 2: Add the Plugin Manager Crate and GitHub Source Parser

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/audiodown-plugin-manager/Cargo.toml`
- Create: `crates/audiodown-plugin-manager/src/lib.rs`
- Create: `crates/audiodown-plugin-manager/src/github.rs`
- Test: `crates/audiodown-plugin-manager/tests/github_source.rs`

- [ ] **Step 1: Write failing GitHub source tests**

Cover canonical parsing and all rejected forms:

```rust
use audiodown_plugin_manager::github::GitHubRepositoryRef;

#[test]
fn accepts_only_canonical_public_repository_urls() {
    let source = GitHubRepositoryRef::parse(
        "https://github.com/example-owner/example-repository",
    )
    .unwrap();
    assert_eq!(source.owner(), "example-owner");
    assert_eq!(source.repository(), "example-repository");
    assert_eq!(
        source.canonical_url(),
        "https://github.com/example-owner/example-repository"
    );
}

#[test]
fn rejects_tokens_subpaths_queries_fragments_and_other_hosts() {
    for value in [
        "http://github.com/owner/repo",
        "https://user:token@github.com/owner/repo",
        "https://github.com/owner/repo/tree/main",
        "https://github.com/owner/repo?tab=readme",
        "https://github.com/owner/repo#readme",
        "https://api.github.com/repos/owner/repo",
        "https://example.invalid/owner/repo",
    ] {
        assert!(GitHubRepositoryRef::parse(value).is_err(), "{value}");
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-manager --test github_source
```

Expected: FAIL because the crate is not a workspace member.

- [ ] **Step 3: Create the crate and strict URL parser**

Add the workspace member and dependencies:

```toml
audiodown-plugin-manager = { path = "crates/audiodown-plugin-manager" }
flate2 = "1.1"
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls", "stream"] }
tar = "0.4"
```

Implement `GitHubRepositoryRef::parse` with `url::Url`, exact host
`github.com`, HTTPS, no username/password/port/query/fragment, exactly two
non-empty path segments, optional trailing slash, and owner/repository segments
matching `[A-Za-z0-9_.-]{1,100}`. Strip a terminal `.git` before producing the
canonical URL.

Define a source abstraction in `lib.rs`:

```rust
#[async_trait::async_trait]
pub trait RepositorySource: Send + Sync {
    async fn resolve_and_download(
        &self,
        source: &github::GitHubRepositoryRef,
        destination: &std::path::Path,
    ) -> Result<DownloadedSnapshot, PluginManagerError>;
}

pub struct DownloadedSnapshot {
    pub commit_sha: String,
    pub archive_path: std::path::PathBuf,
}
```

- [ ] **Step 4: Run parser tests and workspace checks**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test github_source && cargo fmt --all -- --check'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/audiodown-plugin-manager
git commit -m "阶段2：添加 GitHub 仓库地址解析" \
  -m "新增公共 GitHub 仓库地址解析和插件管理骨架。"
```

### Task 3: Resolve and Download Immutable GitHub Snapshots

**Files:**
- Modify: `crates/audiodown-plugin-manager/Cargo.toml`
- Modify: `crates/audiodown-plugin-manager/src/github.rs`
- Test: `crates/audiodown-plugin-manager/tests/github_client.rs`

- [ ] **Step 1: Write failing HTTP client tests**

Use a local Axum test server and inject API/archive base URLs. Verify:

```rust
#[tokio::test]
async fn resolves_default_branch_to_commit_before_downloading() {
    // Server records these requests in order:
    // GET /repos/example-owner/example-repository
    // GET /repos/example-owner/example-repository/commits/main
    // GET /example-owner/example-repository/tar.gz/0123456789abcdef0123456789abcdef01234567
    // It returns a two-byte archive body.
    let result = client
        .resolve_and_download(&source, temp.path())
        .await
        .unwrap();
    assert_eq!(result.commit_sha, "0123456789abcdef0123456789abcdef01234567");
    assert_eq!(tokio::fs::read(result.archive_path).await.unwrap(), b"ok");
}
```

Also test that the client rejects redirects, missing default branches,
non-40-character lowercase hexadecimal SHAs, non-success responses, a response
larger than 16 MiB, and any archive URL not constructed from the locked SHA.

- [ ] **Step 2: Run the client test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-manager --test github_client
```

Expected: FAIL because no GitHub HTTP client exists.

- [ ] **Step 3: Implement the bounded client**

Create `GitHubClient::new(api_base, archive_base)` with a reqwest client that:

```rust
reqwest::Client::builder()
    .redirect(reqwest::redirect::Policy::none())
    .user_agent("AudioDown-Core/1.0")
    .connect_timeout(std::time::Duration::from_secs(10))
    .timeout(std::time::Duration::from_secs(60))
    .build()
```

Read streamed archive chunks and fail before writing byte
`16 * 1024 * 1024 + 1`. Write to `$DESTINATION/snapshot.tar.gz.tmp`, call
`sync_all`, then rename to `snapshot.tar.gz`. Never log the full response body
or arbitrary headers.

Add `axum` and `tower` as dev-dependencies for the local HTTP fixture. Tests
bind `127.0.0.1:0` and never call GitHub.

- [ ] **Step 4: Run GitHub source tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-manager --test github_client
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-manager
git commit -m "阶段2：锁定并下载仓库快照" \
  -m "解析默认分支并下载受限的不可变仓库快照。"
```

### Task 4: Extract Repository Archives Without Filesystem Escape

**Files:**
- Create: `crates/audiodown-plugin-manager/src/archive.rs`
- Modify: `crates/audiodown-plugin-manager/src/lib.rs`
- Test: `crates/audiodown-plugin-manager/tests/archive_safety.rs`

- [ ] **Step 1: Write failing malicious archive tests**

Generate tar.gz fixtures in the test and assert distinct errors for:

```text
../escape
/absolute
root/link -> ../../outside
root/hard-link
root/device
two different top-level directories
duplicate normalized paths
case-folded duplicate paths
non-UTF-8 path
2,049 files
a 4 MiB + 1 byte file
64 MiB + 1 byte extracted total
```

The valid case must extract `root/audiodown-repository.json` to
`$DESTINATION/repository/audiodown-repository.json` without preserving archive
ownership, mode, mtime, links, or extended attributes.
It returns:

```rust
pub struct ExtractedSnapshot {
    pub repository_root: PathBuf,
    pub file_count: usize,
    pub extracted_bytes: u64,
}
```

- [ ] **Step 2: Run the archive test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-manager --test archive_safety
```

Expected: FAIL because `archive::extract_snapshot` is missing.

- [ ] **Step 3: Implement bounded extraction**

Define:

```rust
pub struct SnapshotLimits {
    pub max_compressed_bytes: u64,
    pub max_extracted_bytes: u64,
    pub max_file_bytes: u64,
    pub max_files: usize,
    pub max_plugins: usize,
}

impl Default for SnapshotLimits {
    fn default() -> Self {
        Self {
            max_compressed_bytes: 16 * 1024 * 1024,
            max_extracted_bytes: 64 * 1024 * 1024,
            max_file_bytes: 4 * 1024 * 1024,
            max_files: 2_048,
            max_plugins: 32,
        }
    }
}
```

Use `spawn_blocking`, `flate2::read::GzDecoder`, and `tar::Archive`. Accept only
regular files and directories. Normalize every path component manually, remove
exactly one common top-level component, create files with mode `0600` and
directories with `0700`, and write each file through `create_new(true)`.
Reject both `/` and `\` as escape syntax in index/plugin paths even though tar
paths themselves use `/`.

- [ ] **Step 4: Run archive and clippy checks**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test archive_safety && cargo clippy -p audiodown-plugin-manager --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-manager
git commit -m "阶段2：安全解压插件仓库快照" \
  -m "安全解压仓库快照并拒绝越界及超限内容。"
```

### Task 5: Validate Repository Indexes, Manifests, and npm Lockfiles

**Files:**
- Create: `crates/audiodown-plugin-manager/src/package.rs`
- Create: `crates/audiodown-plugin-manager/src/validation.rs`
- Modify: `crates/audiodown-plugin-manager/src/lib.rs`
- Test: `crates/audiodown-plugin-manager/tests/repository_validation.rs`

- [ ] **Step 1: Write failing repository validation tests**

Create a valid dependency-free repository and assert:

```rust
let validated = validate_repository(
    repository_root,
    &Version::parse("1.0.0-alpha.1").unwrap(),
    &Version::parse("1.0.0").unwrap(),
    SnapshotLimits::default(),
)
.unwrap();
assert_eq!(validated.repository_id, "example.plugins");
assert_eq!(validated.plugins.len(), 1);
assert_eq!(
    validated.plugins[0].manifest.id.as_str(),
    "com.audiodown.virtual.content"
);
assert_eq!(validated.plugins[0].entry_path, "src/index.js");
```

Add one test per rejection:

```text
unknown schemaVersion
repository ID outside lowercase ASCII letters, digits, `.`, `_`, and `-`
empty repository name or repository name longer than 120 characters
empty or duplicate plugin paths
path traversal or absolute plugin path
more than 32 plugins
manifest ID duplicated in one repository
index path does not resolve to a directory containing audiodown-plugin.json
Node runtime other than 22
absolute or parent-directory entry path
entry file missing
invalid core or pluginApi VersionReq
major-version incompatibility
malformed or duplicate capability
invalid allowedHosts entry, IP literal, localhost, or wildcard outside a leading `*.`
lifecycle scripts required without a non-empty reason <= 240 characters
package lifecycle script present while manifest declares required=false
manifest declares lifecycle scripts but package has no
preinstall/install/postinstall/prepublish/preprepare/prepare/postprepare script
package.json or package-lock.json missing
lockfileVersion below 2
more than 256 locked packages
file:, link:, git, GitHub shorthand, workspace:, HTTP, or non-registry resolved dependencies
resolved URL with credentials, query, fragment, non-HTTPS, or host other than registry.npmjs.org
missing or invalid sha512 integrity for a remote package
.npmrc, npm-shrinkwrap.json, yarn.lock, pnpm-lock.yaml, nested package-manager config,
Dockerfile, or .dockerignore anywhere in the plugin tree
```

- [ ] **Step 2: Run validation tests and verify they fail**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-manager --test repository_validation
```

Expected: FAIL because validation modules are missing.

- [ ] **Step 3: Implement deterministic validation**

Return:

```rust
pub struct ValidatedRepository {
    pub repository_id: String,
    pub repository_name: String,
    pub plugins: Vec<ValidatedPlugin>,
}

pub struct ValidatedPlugin {
    pub relative_path: String,
    pub manifest: PluginManifest,
    pub manifest_hash: String,
    pub source_hash: String,
    pub entry_path: String,
    pub requires_lifecycle_scripts: bool,
    pub lifecycle_script_reason: Option<String>,
}
```

Hash files in sorted normalized-path order using:

```text
u64 big-endian path length
UTF-8 path bytes
u64 big-endian content length
file bytes
```

Normalize `1.0.0-alpha.1` to `1.0.0` only for compatibility matching; keep the
actual Core version in API responses. Parse npm integrity strings as
`sha512-{base64}` and reject any other algorithm.

Validate capability names with
`^[a-z][a-z0-9]*(\.[a-z][a-z0-9]*)+$` without assigning runtime semantics in
this phase. Validate allowed hosts as lowercase DNS names with an optional
single leading `*.`; runtime still has no network path.

- [ ] **Step 4: Run all plugin manager tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager && cargo clippy -p audiodown-plugin-manager --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-manager
git commit -m "阶段2：校验插件仓库内容" \
  -m "校验仓库索引、插件清单和 npm 锁文件。"
```

### Task 6: Persist Snapshot Metadata, Plugin Settings, and Risk Grants

**Files:**
- Create: `migrations/0002_plugin_installation.sql`
- Modify: `crates/audiodown-domain/src/plugin.rs`
- Modify: `crates/audiodown-storage/src/lib.rs`
- Modify: `crates/audiodown-storage/src/plugin_repository.rs`
- Create: `crates/audiodown-storage/src/risk_grant_repository.rs`
- Test: `crates/audiodown-storage/tests/plugin_installation.rs`

- [ ] **Step 1: Write failing storage tests**

The test must:

```rust
let operation_id = Uuid::new_v4();
storage.plugins().set_install_result(
    &plugin_id,
    "example.plugins",
    "sha256:image",
    "source-tree-sha256",
    operation_id,
    PluginStatus::Installing,
).await?;
storage.plugins().update_settings(
    &plugin_id,
    false,
    RunMode::Always,
    25,
).await?;
storage.plugins().touch(&plugin_id, now).await?;

let grant = RiskGrantRecord {
    id: Uuid::new_v4(),
    repository_id: "example.plugins".into(),
    plugin_id: plugin_id.clone(),
    commit_sha: "0123456789abcdef0123456789abcdef01234567".into(),
    risk_kind: "npm_lifecycle_scripts".into(),
    reason: "Generate a deterministic local file".into(),
    granted_at: now,
};
storage.risk_grants().insert(&grant).await?;
assert!(storage.risk_grants().exists_for(
    &plugin_id,
    &grant.commit_sha,
    "npm_lifecycle_scripts",
).await?);

storage.plugins().delete(&plugin_id).await?;
assert!(storage.plugins().get(&plugin_id).await?.is_none());
```

- [ ] **Step 2: Run the storage test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-storage --test plugin_installation
```

Expected: FAIL because the migration and repository methods do not exist.

- [ ] **Step 3: Add the migration and repositories**

Migration:

```sql
ALTER TABLE plugins ADD COLUMN repository_id TEXT;
ALTER TABLE plugins ADD COLUMN source_hash TEXT;
ALTER TABLE plugins ADD COLUMN last_used_at TEXT;
ALTER TABLE plugins ADD COLUMN install_operation_id TEXT;

CREATE TABLE plugin_risk_grants (
  id TEXT PRIMARY KEY,
  repository_id TEXT NOT NULL,
  plugin_id TEXT NOT NULL,
  commit_sha TEXT NOT NULL,
  risk_kind TEXT NOT NULL,
  reason TEXT NOT NULL,
  granted_at TEXT NOT NULL,
  UNIQUE(plugin_id, commit_sha, risk_kind)
);

CREATE INDEX idx_plugin_risk_grants_plugin
  ON plugin_risk_grants(plugin_id, commit_sha);
```

Keep `priority` in `0..=1000`. `set_install_result`, `update_settings`, `touch`,
and `delete` must return `StorageError::NotFound` when `rows_affected() != 1`.
Deleting a plugin does not delete its risk grants because grants are audit
records tied to a historical commit.

Add `PluginStatus::Installing` and update all storage codecs. Extend
`PluginRecord` with nullable `repository_id`, `source_hash`,
`install_operation_id`, and `last_used_at`; update every existing constructor
and fixture compatibility test. Add `list_pending_install_operations()` and
conditional `complete_install(plugin_id, operation_id)` and
`rollback_install(plugin_id, operation_id)` methods so restart reconciliation
cannot complete or remove a newer operation accidentally.

- [ ] **Step 4: Run storage and migration tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-storage && cargo clippy -p audiodown-storage --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add migrations crates/audiodown-domain crates/audiodown-storage
git commit -m "阶段2：持久化插件安装状态" \
  -m "持久化插件来源、设置、安装操作和风险授权。"
```

### Task 7: Stage Validated Repository Snapshots

**Files:**
- Create: `crates/audiodown-plugin-manager/src/staging.rs`
- Modify: `crates/audiodown-plugin-manager/src/lib.rs`
- Test: `crates/audiodown-plugin-manager/tests/staging.rs`

- [ ] **Step 1: Write failing staging tests**

Assert that `SnapshotStore::create` writes:

```text
/data/plugins/staging/65ddab42-9e2f-4de1-a159-705bf9d055e9/repository/...
/data/plugins/staging/65ddab42-9e2f-4de1-a159-705bf9d055e9/snapshot.json
```

and `prepare_install` writes only:

```json
{
  "schemaVersion": "1.0",
  "operationId": "75de0d58-03f9-4db7-8a27-69ac7ddce8de",
  "snapshotId": "65ddab42-9e2f-4de1-a159-705bf9d055e9",
  "pluginId": "com.audiodown.virtual.content",
  "repositoryId": "example.plugins",
  "sourceUrl": "https://github.com/example-owner/example-repository",
  "commitSha": "0123456789abcdef0123456789abcdef01234567",
  "pluginPath": "plugins/virtual-content",
  "manifestHash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "sourceHash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "allowLifecycleScripts": false,
  "riskGrantId": null
}
```

Use atomic temporary files and assert modes `0600` for metadata and `0700` for
directories on Unix. Test that snapshot and operation IDs must be UUIDs and
that cleanup removes staging entries older than 30 minutes but never traverses
symlinks.

For a granted lifecycle-script build, `prepare_install` must also mirror the
database grant to
`/data/plugins/grants/85df1e67-a533-42dc-81e1-7b18687840fe.json` with plugin
ID, repository ID, commit SHA, risk kind, reason, and granted timestamp. The
prepared operation refers only to that UUID. Supervisor revalidates the mirror
and deletes it after the operation; SQLite remains the durable audit record.

- [ ] **Step 2: Run staging tests and verify they fail**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-manager --test staging
```

Expected: FAIL because `SnapshotStore` is missing.

- [ ] **Step 3: Implement staging metadata**

Expose:

```rust
pub struct SnapshotStore {
    plugin_data: PathBuf,
}

impl SnapshotStore {
    pub async fn create(
        &self,
        source: &GitHubRepositoryRef,
        commit_sha: &str,
        extracted: ExtractedSnapshot,
        validated: ValidatedRepository,
    ) -> Result<RepositoryPreview, PluginManagerError>;

    pub async fn prepare_install(
        &self,
        snapshot_id: Uuid,
        plugin_id: &PluginId,
        grant: Option<&LifecycleRiskGrant>,
    ) -> Result<PreparedOperation, PluginManagerError>;
}
```

Derive all filesystem paths from validated UUID/plugin IDs. Never accept a
caller-provided filesystem path. Define `LifecycleRiskGrant` in `staging.rs`
with `id`, `repository_id`, `plugin_id`, `commit_sha`, `risk_kind`, `reason`,
and `granted_at`; the plugin manager must not depend on the storage crate.

Define the complete staging outputs in the same module:

```rust
pub struct RepositoryPreview {
    pub snapshot_id: Uuid,
    pub repository_id: String,
    pub repository_name: String,
    pub source_url: String,
    pub commit_sha: String,
    pub plugins: Vec<PluginPreview>,
}

pub struct PluginPreview {
    pub plugin_id: PluginId,
    pub name: String,
    pub version: Version,
    pub plugin_type: PluginType,
    pub requires_lifecycle_script_grant: bool,
    pub lifecycle_script_reason: Option<String>,
}

pub struct PreparedOperation {
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
}
```

- [ ] **Step 4: Run plugin manager tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-plugin-manager
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-manager
git commit -m "阶段2：暂存已校验插件快照" \
  -m "暂存已验证快照和受约束的安装操作元数据。"
```

### Task 8: Expose Repository Inspection Through Core

**Files:**
- Modify: `crates/audiodown-plugin-manager/Cargo.toml`
- Create: `crates/audiodown-plugin-manager/src/service.rs`
- Modify: `crates/audiodown-plugin-manager/src/lib.rs`
- Modify: `crates/audiodown-server/Cargo.toml`
- Modify: `crates/audiodown-server/src/config.rs`
- Modify: `crates/audiodown-server/src/state.rs`
- Modify: `crates/audiodown-server/src/app.rs`
- Modify: `crates/audiodown-server/src/lib.rs`
- Modify: `crates/audiodown-server/src/main.rs`
- Create: `crates/audiodown-server/src/plugin_manager_adapters.rs`
- Create: `crates/audiodown-server/src/routes/repositories.rs`
- Modify: `crates/audiodown-server/src/routes/mod.rs`
- Modify: `docker-compose.yml`
- Test: `crates/audiodown-plugin-manager/tests/inspection_service.rs`
- Test: `crates/audiodown-server/tests/repository_api.rs`

- [ ] **Step 1: Write failing inspection service and API tests**

Use fake `RepositorySource` and `PluginStateStore` ports to prove the manager
service owns fetch, extraction, validation, staging, stale cleanup, installed
lookup, and the two-permit concurrency limit. Then inject the service into Axum
and call:

```http
POST /api/v1/plugin-repositories/inspect
Content-Type: application/json

{"url":"https://github.com/example-owner/example-repository"}
```

Assert `200`:

```json
{
  "snapshotId": "65ddab42-9e2f-4de1-a159-705bf9d055e9",
  "repository": {
    "id": "example.plugins",
    "name": "Example Plugins",
    "sourceUrl": "https://github.com/example-owner/example-repository",
    "commitSha": "0123456789abcdef0123456789abcdef01234567"
  },
  "plugins": [{
    "pluginId": "com.audiodown.virtual.content",
    "name": "Virtual Content",
    "version": "1.0.0",
    "pluginType": "content",
    "alreadyInstalled": false,
    "requiresLifecycleScriptGrant": false,
    "lifecycleScriptReason": null
  }]
}
```

Assert stable errors for invalid URL (`INVALID_REPOSITORY_URL`), download
failure (`REPOSITORY_UNAVAILABLE`), invalid archive (`INVALID_REPOSITORY`), and
Supervisor unavailability does not block inspection.

Start three blocked fake inspections and assert only two enter the source
client; the third receives `429 REPOSITORY_INSPECTION_BUSY`.

- [ ] **Step 2: Run the API test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test inspection_service && cargo test -p audiodown-server --test repository_api'
```

Expected: FAIL because the service and route do not exist.

- [ ] **Step 3: Wire the repository service and route**

Create `PluginManagerService` in `audiodown-plugin-manager`; it owns an
`Arc<dyn PluginStateStore>`, `Arc<dyn RepositorySource>`, `SnapshotStore`, and
the inspection semaphore. Define the minimal manager-owned `PluginStateStore`
port needed for installed-state lookup; do not make the manager crate depend
on `audiodown-storage`. Runtime control is not added until Task 12.

Create `SqlitePluginManagerStore` in
`audiodown-server/src/plugin_manager_adapters.rs` as a newtype over the
existing storage handle and implement `PluginStateStore` there. The service,
not the Axum route, performs stale-snapshot cleanup, fetch, extraction,
validation, staging, installed-state lookup, and stable error mapping.

Add `Arc<PluginManagerService>` to `AppState`. Preserve `AppState::new` for
existing tests by installing an unavailable repository source/runtime adapter,
and add a `with_plugin_manager` builder used by production startup and
repository API tests. Production config defaults:

```text
AUDIODOWN_GITHUB_API_BASE=https://api.github.com
AUDIODOWN_GITHUB_ARCHIVE_BASE=https://codeload.github.com
```

Non-default bases are legal only when `AUDIODOWN_DEV_MODE=1`; otherwise startup
must fail. The route body uses `#[serde(deny_unknown_fields)]`, caps URL length
at 512 bytes, then calls only `PluginManagerService::inspect_repository`.

Pass both base URLs through Compose with the production defaults so integration
tests can override them without adding another user-managed service.
Guard fetch/extract/validate with the two-permit semaphore owned by the manager
service.

- [ ] **Step 4: Run server API and clippy checks**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test inspection_service && cargo test -p audiodown-server --test repository_api && cargo clippy -p audiodown-plugin-manager --all-targets -- -D warnings && cargo clippy -p audiodown-server --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/audiodown-plugin-manager crates/audiodown-server docker-compose.yml
git commit -m "阶段2：开放插件仓库检查接口" \
  -m "通过薄 API 暴露公共插件仓库检查能力。"
```

### Task 9: Add the Shared Idempotent Supervisor Operation Protocol

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/audiodown-supervisor-protocol/Cargo.toml`
- Create: `crates/audiodown-supervisor-protocol/src/lib.rs`
- Modify: `crates/audiodown-supervisor/Cargo.toml`
- Modify: `crates/audiodown-supervisor/src/protocol.rs`
- Modify: `crates/audiodown-supervisor/src/server.rs`
- Modify: `crates/audiodown-server/Cargo.toml`
- Modify: `crates/audiodown-server/src/supervisor.rs`
- Test: `crates/audiodown-supervisor-protocol/tests/contracts.rs`
- Test: `crates/audiodown-supervisor/tests/protocol.rs`
- Test: `crates/audiodown-server/tests/supervisor_client.rs`

- [ ] **Step 1: Write failing protocol and result tests**

Accepted build request:

```json
{
  "id": "req-1",
  "token": "token",
  "timestamp": 1,
  "nonce": "nonce",
  "method": "plugin.install.build",
  "params": {
    "pluginId": "com.audiodown.virtual.content",
    "operationId": "75de0d58-03f9-4db7-8a27-69ac7ddce8de"
  }
}
```

The same exact params shape is used for `plugin.install.status`,
`plugin.install.finalize`, `plugin.install.abort`, and `plugin.install.ack`.
`plugin.install.list` accepts no caller-controlled filter and returns only
operations owned by the current installation ID. Accepted remove requests
contain only `pluginId`. Reject install params containing `image`, `dockerfile`,
`command`, `buildArgs`, `network`, `mounts`, `environment`, `sourcePath`,
`repositoryUrl`, or `allowScripts`.

Round-trip these complete operation types:

```rust
pub enum PluginInstallOperationState {
    Accepted,
    Building,
    Built,
    Finalized,
    Failed,
    Aborted,
}

pub struct PluginInstallArtifact {
    pub image_id: String,
    pub repository_id: String,
    pub commit_sha: String,
    pub source_hash: String,
    pub manifest_hash: String,
}

pub struct PluginInstallOperation {
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
    pub state: PluginInstallOperationState,
    pub artifact: Option<PluginInstallArtifact>,
    pub build_logs: Vec<PluginBuildLog>,
    pub error_code: Option<String>,
    pub acknowledged: bool,
}

pub struct PluginInstallOperationSummary {
    pub operation_id: Uuid,
    pub plugin_id: PluginId,
    pub state: PluginInstallOperationState,
    pub artifact: Option<PluginInstallArtifact>,
    pub error_code: Option<String>,
    pub acknowledged: bool,
}

pub struct PluginInstallOperationList {
    pub operations: Vec<PluginInstallOperationSummary>,
}

pub struct PluginRemoveResult {
    pub plugin_id: PluginId,
    pub removed_container: bool,
    pub removed_image: bool,
    pub removed_install_directory: bool,
}
```

Assert list responses are capped at 256, contain only the authenticated
installation ID, and reject unknown fields. Assert terminal operations cannot
be removed by retention until a matching `ack` succeeds.

- [ ] **Step 2: Run the protocol test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-supervisor-protocol --test contracts
```

Expected: FAIL because the shared protocol crate does not exist.

- [ ] **Step 3: Implement the shared contracts and method validation**

Move request/response/method/result wire types into the shared crate. Supported
methods are:

```text
system.ping
plugin.start
plugin.stop
plugin.inspect
plugin.logs
plugin.install.build
plugin.install.status
plugin.install.finalize
plugin.install.abort
plugin.install.list
plugin.install.ack
plugin.remove
```

`plugin.install.build` is idempotent: the first call records `accepted` and
starts one background operation; repeated calls with the same operation/plugin
return current status, while reuse with another plugin returns
`OPERATION_ID_MISMATCH`. All protocol calls remain short and use the existing
two-second timeout. Core polls status every 500 ms for at most 10 minutes.
`plugin.install.list` returns at most 256 non-acknowledged operations in stable
creation order, contains no build logs, and never exposes another installation
ID. Core calls `status` only for summaries that require detailed
reconciliation. `ack` is accepted only for terminal `finalized`, `failed`, or
`aborted` operations.

Add Core client methods:

```rust
async fn begin_plugin_install(
    &self,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> Result<PluginInstallOperation, SupervisorError>;

async fn plugin_install_status(
    &self,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> Result<PluginInstallOperation, SupervisorError>;

async fn finalize_plugin_install(
    &self,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> Result<PluginInstallOperation, SupervisorError>;

async fn abort_plugin_install(
    &self,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> Result<PluginInstallOperation, SupervisorError>;

async fn list_plugin_install_operations(
    &self,
) -> Result<PluginInstallOperationList, SupervisorError>;

async fn acknowledge_plugin_install(
    &self,
    plugin_id: &PluginId,
    operation_id: Uuid,
) -> Result<PluginInstallOperation, SupervisorError>;

async fn remove_plugin(
    &self,
    plugin_id: &PluginId,
) -> Result<PluginRemoveResult, SupervisorError>;
```

Protocol errors have an optional bounded `details` value. Build failures may
use only `{"buildLogs":[...]}`; Core never returns those details directly and
redacts them before persistence.

- [ ] **Step 4: Run shared protocol, Supervisor, and client tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-supervisor-protocol && cargo test -p audiodown-supervisor --test protocol && cargo test -p audiodown-server --test supervisor_client'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/audiodown-supervisor-protocol crates/audiodown-supervisor crates/audiodown-server
git commit -m "阶段2：添加 Supervisor 幂等操作协议" \
  -m "定义 Core 与 Supervisor 共用的幂等安装操作协议。"
```

### Task 10: Add the Isolated npm Build Proxy

**Files:**
- Create: `crates/audiodown-supervisor/src/build_proxy.rs`
- Modify: `crates/audiodown-supervisor/src/lib.rs`
- Modify: `crates/audiodown-supervisor/src/main.rs`
- Modify: `crates/audiodown-supervisor/Cargo.toml`
- Test: `crates/audiodown-supervisor/tests/build_proxy.rs`

- [ ] **Step 1: Write failing proxy policy tests**

Start the proxy with an injected resolver/connector and assert:

```text
CONNECT registry.npmjs.org:443 -> accepted
CONNECT registry.npmjs.org:80 -> rejected
CONNECT sub.registry.npmjs.org:443 -> rejected
CONNECT localhost:443 -> rejected
CONNECT 127.0.0.1:443 -> rejected
CONNECT [::1]:443 -> rejected
CONNECT registry.npmjs.org:443 when DNS returns private/link-local/loopback -> rejected
GET http://registry.npmjs.org/... -> rejected
request headers over 16 KiB -> rejected
more than 32 concurrent tunnels -> rejected
one tunnel over 64 MiB -> closed
aggregate proxy traffic over 256 MiB -> proxy exits non-zero
idle tunnel after 60 seconds -> closed
```

- [ ] **Step 2: Run the proxy test and verify it fails**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  cargo test -p audiodown-supervisor --test build_proxy
```

Expected: FAIL because `build_proxy` is missing.

- [ ] **Step 3: Implement the fixed CONNECT proxy**

Add a `build-proxy` process mode:

```rust
match std::env::args().nth(1).as_deref() {
    Some("build-proxy") => build_proxy::run().await,
    Some(_) => anyhow::bail!("unknown Supervisor mode"),
    None => run_supervisor().await,
}
```

The proxy listens on `0.0.0.0:18081`, parses one HTTP/1.1 request, accepts only
the exact authority `registry.npmjs.org:443`, resolves all addresses before
connecting, rejects any non-global address, returns `200 Connection
Established`, then copies bytes bidirectionally with the concurrency and idle
limits above. Use injected limits and paused time in tests; do not sleep for 60
real seconds. It never logs tunneled bytes or headers.

- [ ] **Step 4: Run proxy and Supervisor tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-supervisor --test build_proxy && cargo clippy -p audiodown-supervisor --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/audiodown-supervisor
git commit -m "阶段2：添加隔离 npm 构建代理" \
  -m "新增仅允许固定 npm 主机的隔离构建代理。"
```

### Task 11: Build and Finalize Managed Node Plugin Images

**Files:**
- Modify: `Cargo.lock`
- Create: `crates/audiodown-supervisor/src/prepared_install.rs`
- Create: `crates/audiodown-supervisor/src/builder.rs`
- Create: `crates/audiodown-supervisor/src/trusted_images.rs`
- Modify: `crates/audiodown-supervisor/Cargo.toml`
- Modify: `crates/audiodown-supervisor/src/config.rs`
- Modify: `crates/audiodown-supervisor/src/docker.rs`
- Modify: `crates/audiodown-supervisor/src/install_record.rs`
- Modify: `crates/audiodown-supervisor/src/lib.rs`
- Modify: `crates/audiodown-supervisor/src/server.rs`
- Modify: `docker/supervisor.Dockerfile`
- Create: `docker/plugin-runtime/node22.lock.json`
- Create: `docker/plugin-runtime/node22-builder.Dockerfile`
- Create: `docker/plugin-runtime/node22-runtime.Dockerfile`
- Create: `docker/plugin-runtime/node22-build-runner.js`
- Modify: `docker-compose.yml`
- Test: `crates/audiodown-supervisor/tests/prepared_install.rs`
- Test: `crates/audiodown-supervisor/tests/build_policy.rs`
- Test: `crates/audiodown-supervisor/tests/install_operation.rs`

- [ ] **Step 1: Write failing prepared-operation tests**

Test that Supervisor derives
`/data/plugins/prepared/75de0d58-03f9-4db7-8a27-69ac7ddce8de.json`
itself, then rejects:

```text
operation ID or plugin ID mismatch
unknown schema version
missing snapshot directory
pluginPath traversal or symlink
manifest/source hash mismatch
commit SHA or repository ID mismatch
riskGrantId missing when allowLifecycleScripts=true
allowLifecycleScripts=true without a matching mirrored grant
runtime other than Node.js 22
dependency resolved outside registry.npmjs.org
```

The valid request returns a `ValidatedPreparedInstall` containing no
caller-provided image name, command, network, mount, resource limit, build
container, runtime container, or environment variable.

- [ ] **Step 2: Write failing trusted-image and build-policy tests**

Assert:

```text
node22.lock.json contains an exact node:22-bookworm-slim sha256 digest
Supervisor pulls only image@digest and verifies RepoDigests
trusted builder/runtime image labels contain base digest, SDK hash, and policy version
one global build permit and one operation per plugin
image tag matches ^audiodown/plugin-[0-9a-f]{12}:0123456789ab-bbbbbbbbbbbb$
builder joins only an internal build network with proxy alias audiodown-npm-proxy
proxy alone also joins a separate operation-scoped egress bridge
HTTPS_PROXY=http://audiodown-npm-proxy:18081
builder runs as fixed uid:gid 10001:10001 with cap_drop=ALL and no-new-privileges
builder has no bind mounts, Docker Socket, devices, privileged mode, or Host network
builder uses read-only rootfs plus /workspace tmpfs size 268435456
builder has 512 MiB memory, 1 CPU, 128 PIDs, and 5-minute timeout
proxy runs as uid:gid 10002:10002 with no mounts or Docker Socket, read-only
rootfs, cap_drop=ALL, no-new-privileges, 128 MiB memory, 0.5 CPU, and 64 PIDs
builder log output over 1 MiB terminates with BUILD_LOG_LIMIT_EXCEEDED
npm ci uses --omit=dev --ignore-scripts --no-audit --no-fund by default
npm ci omits --ignore-scripts only for a validated mirrored grant
assembler never starts untrusted code and has network disabled
build output rejects absolute/traversal/duplicate paths, hard links, devices, FIFOs, and escaping symlinks
validated build output is repacked into a normalized archive before assembler put_archive
repacked output uses root:root, directories 0755, files 0644, and no extended metadata
final image labels contain installation ID, plugin ID, commit SHA, source hash,
manifest hash, base digest, SDK hash, and managed-image marker
```

- [ ] **Step 3: Write failing operation-state tests**

Using a fake Docker adapter, prove:

```text
first build call records accepted and starts one background task
repeated build call with same operation is idempotent
second operation while one build is active returns BUILD_BUSY
status reports accepted -> building -> built
built operation owns a candidate directory and image but no installed directory
finalize atomically promotes candidate to installed and reports finalized
repeated finalize is idempotent
crash after candidate rename but before finalized state recovers from a matching installed attestation
crash after finalized state but before mirror cleanup completes cleanup idempotently
installed attestation mismatch never overwrites the directory or reports finalized
abort removes candidate, image, prepared request, and mirrored grant
Supervisor restart changes orphaned building to failed and cleans temporary resources
unacknowledged terminal status survives restart without retention cleanup
acknowledged failed/finalized/aborted status remains queryable for 30 minutes
```

- [ ] **Step 4: Run the tests and verify they fail**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-supervisor --test prepared_install && cargo test -p audiodown-supervisor --test build_policy && cargo test -p audiodown-supervisor --test install_operation'
```

Expected: FAIL because prepared install, trusted image, builder, and operation
modules are missing.

- [ ] **Step 5: Pin the Node base image**

Run:

```bash
docker buildx imagetools inspect node:22-bookworm-slim \
  --format '{{json .Manifest.Digest}}'
```

Expected: one quoted `sha256:` digest with 64 lowercase hexadecimal
characters. Record that digest in `node22.lock.json` together with
`node:22-bookworm-slim`. Do not use a tag without the committed digest.

- [ ] **Step 6: Implement the restricted builder and assembler**

Supervisor first ensures two trusted images from fixed embedded Dockerfiles:

```text
audiodown/plugin-builder-node22:1.0
audiodown/plugin-runtime-node22:1.0
```

It performs a controlled pull of the committed `image@digest`, verifies the
digest, builds only the trusted Dockerfiles/SDK/runner, and verifies trusted
image labels. Untrusted repository files are never part of a Docker build
context.

For each operation:

```text
1. Atomically persist accepted operation state.
2. Revalidate the prepared operation and staged source tree.
3. Acquire the global build permit.
4. Create an operation-scoped internal build network and a separate egress
   bridge. Attach only the fixed proxy sidecar to both; attach the builder only
   to the internal network.
5. Create/start the trusted builder as uid:gid 10001:10001 with
   cap_drop=ALL, no-new-privileges, read-only rootfs, a writable
   10001:10001-owned /workspace tmpfs, fixed resource limits, fixed runner,
   and no host mounts.
6. Stream the validated plugin source archive into /workspace/input through
   Docker put_archive with normalized uid:gid 10001:10001 and safe modes; the
   final tar entry is .input-ready.
7. The fixed runner copies input to output, executes one of the two exact npm
   commands, writes status.json, and waits without running plugin entry code.
8. Poll status, enforce the five-minute deadline, and capture at most 1 MiB of
   ordered stdout/stderr. If the limit is reached, stop the builder, fail with
   BUILD_LOG_LIMIT_EXCEEDED, and persist the captured output plus a terminal
   truncation event; never silently discard output and continue the build.
9. Stream /workspace/output back through get_archive and enforce the 256 MiB
   limit. Parse the returned tar as untrusted input, enforce normalized
   relative paths, duplicate/file-count/per-file/total-size limits, regular
   files/directories, and only relative symlinks that resolve inside the
   output tree. Reject hard links, absolute or escaping links, devices, FIFOs,
   and unknown entry types, then generate a fresh normalized tar owned by
   root:root. Normalize directories to `0755`, regular files to `0644`, and
   safe symlinks to `0777`; clear setuid, setgid, sticky, xattrs, ACLs,
   capabilities, and all other extended metadata.
10. Create a stopped assembler container from the trusted runtime image,
    network disabled, and put_archive only the normalized output into /plugin.
11. Commit the never-started assembler to the deterministic managed image and
    verify labels.
12. Write candidates/<operation-id>/audiodown-plugin.json and install.json,
    then persist built state with artifact metadata.
13. Always remove builder, assembler, proxy, and network.
```

`finalize` fsyncs and atomically renames the candidate directory to
`installed/com.audiodown.virtual.content`, fsyncs the parent, persists
`finalized`, then removes prepared/grant mirrors. On retry or restart, a
missing candidate plus a matching installed attestation reconstructs
`finalized`; any installation ID, operation ID, plugin ID, or artifact hash
mismatch is a hard failure and is never overwritten. A finalized operation
with leftover mirrors completes cleanup idempotently. `abort` removes all
operation-owned resources. Only Supervisor deletes submitted prepared/grant
files.

Unacknowledged terminal operation state is retained across restarts.
Acknowledged terminal state may be removed after 30 minutes. Refuse new
operations once 256 unacknowledged records exist so disk state remains bounded.

The install record adds `operationId`, `sourceKind`, `sourceHash`, `commitSha`,
`repositoryId`, `baseImageDigest`, and `sdkHash`. It is a runtime attestation,
not the owner of plugin settings. Repository images must pass managed-image
label inspection before runtime start.

Copy fixed Dockerfiles, lock data, runner, and SDK into the Supervisor image.
Compose explicitly sets:

```text
AUDIODOWN_SUPERVISOR_IMAGE=audiodown/supervisor:1.0.0-alpha.1
```

The proxy sidecar image is fixed by Supervisor config and never comes from an
install request.

- [ ] **Step 7: Run Supervisor tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-supervisor && cargo clippy -p audiodown-supervisor --all-targets -- -D warnings'
```

Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add Cargo.lock crates/audiodown-supervisor docker docker-compose.yml
git commit -m "阶段2：构建受管 Node 插件镜像" \
  -m "使用受限构建容器和固定运行时组装托管插件镜像。"
```

### Task 12: Complete the Two-Phase Install and Risk Grant Flow

**Files:**
- Modify: `Cargo.lock`
- Modify: `crates/audiodown-plugin-manager/Cargo.toml`
- Modify: `crates/audiodown-plugin-manager/src/service.rs`
- Modify: `crates/audiodown-plugin-manager/src/lib.rs`
- Modify: `crates/audiodown-server/Cargo.toml`
- Modify: `crates/audiodown-server/src/config.rs`
- Modify: `crates/audiodown-server/src/state.rs`
- Modify: `crates/audiodown-server/src/plugin_manager_adapters.rs`
- Modify: `crates/audiodown-server/src/routes/plugins.rs`
- Modify: `crates/audiodown-server/src/routes/system.rs`
- Modify: `crates/audiodown-server/src/app.rs`
- Modify: `crates/audiodown-server/src/main.rs`
- Test: `crates/audiodown-plugin-manager/tests/install_service.rs`
- Test: `crates/audiodown-server/tests/development_config.rs`
- Test: `crates/audiodown-server/tests/plugin_install_api.rs`

- [ ] **Step 1: Write failing manager transaction tests**

With fake storage/runtime adapters, assert:

```text
loads only the staged snapshot
rejects expired/missing snapshots and plugin IDs absent from the snapshot
rejects already-installed plugin with PLUGIN_ALREADY_INSTALLED
wins a per-plugin operation lock
lock-registry cleanup never creates two live locks for the same plugin
writes one prepared operation and optional mirrored grant
begin returns immediately and status polling observes accepted/building/built
build logs are redacted and persisted for success and failure
built artifact hashes must equal staged metadata before SQLite mutation
SQLite row is inserted as installing with install_operation_id before finalize
finalize success changes SQLite to installed and clears install_operation_id
SQLite insertion failure calls abort and leaves no plugin row
finalize transport failure preserves installing row for startup reconciliation
Core restart finalizes installing rows whose Supervisor operation is built
Core restart completes rows whose operation is already finalized
Core restart acknowledges finalized operations matching an already-installed SQLite row
Core restart aborts and conditionally rolls back an installing row whose operation is failed/aborted
Core restart aborts built operations that have no SQLite row
Core restart removes finalized operations that have no SQLite row, then acknowledges them
successful/rolled-back operations are acknowledged only after SQLite and runtime cleanup agree
unacknowledged operations are discovered through plugin.install.list after long Core downtime
only acknowledged terminal operations older than 30 minutes are cleaned
10-minute HTTP wait timeout attempts abort and returns INSTALL_TIMEOUT
```

- [ ] **Step 2: Write failing API and risk-approval tests**

Call:

```http
POST /api/v1/plugin-repositories/65ddab42-9e2f-4de1-a159-705bf9d055e9/plugins/com.audiodown.virtual.content/install
Content-Type: application/json

{"allowLifecycleScripts":false}
```

The Axum route must call `PluginManagerService::install` and contain no
filesystem, SQLite transaction, polling, or Supervisor orchestration logic.
A second concurrent install of the same plugin returns
`409 PLUGIN_OPERATION_IN_PROGRESS`. Cross-operation locking with lifecycle,
settings, and uninstall is added and tested in Task 13.

For a script-requiring manifest, assert `409 RISK_GRANT_REQUIRED` without
approval, `403 DEVELOPER_MODE_REQUIRED` outside developer mode, `401
DEV_TOKEN_REQUIRED` with a bad `x-audiodown-dev-token`, and success with a
valid token. The grant matches plugin ID, repository ID, commit SHA, risk kind,
and declared reason. An existing historical grant never bypasses a fresh
explicit checkbox and valid token. `GET /api/v1/system` adds
`"developmentMode": true|false` to the existing response and never exposes the
token.

Add configuration tests proving both `Config::dev_token` and
`DevelopmentConfig::token` use `SecretString`, their `Debug` output is
redacted, they do not implement response serialization, and startup/logging
paths never emit the configured token.

- [ ] **Step 3: Run manager and API tests and verify they fail**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test install_service && cargo test -p audiodown-server --test development_config && cargo test -p audiodown-server --test plugin_install_api'
```

Expected: FAIL because the two-phase install service and route are absent.

- [ ] **Step 4: Implement the manager-owned transaction**

Extend the manager-owned `PluginStateStore` port with installation transaction,
risk-audit, build-log, and reconciliation methods. Implement those methods in
`SqlitePluginManagerStore`; the manager crate still must not depend directly on
`audiodown-storage`.

Define `PluginRuntimeControl` in the manager crate with
start/stop/inspect/remove and the begin/status/finalize/abort/list/ack methods
from Task 9. Implement it for `UnixSupervisorClient` in the server adapter
module.

Define a manager-owned `LifecycleRiskAuthorizer` port. Convert both
`Config::dev_token` and `DevelopmentConfig::token` to
`secrecy::SecretString`. Its production server adapter reads that secret
configuration and compares the supplied token in constant time. The route
never constructs a trusted/granted enum; it only wraps the optional header as
`SecretString` and passes the user's explicit body choice to the manager. Add:

```rust
pub struct InstallPluginCommand {
    pub snapshot_id: Uuid,
    pub plugin_id: PluginId,
    pub lifecycle_risk: LifecycleRiskInput,
}

pub struct LifecycleRiskInput {
    pub explicitly_approved: bool,
    pub developer_token: Option<SecretString>,
}
```

The manager loads the staged manifest first. If lifecycle scripts are required,
it requires `explicitly_approved=true` and asks `LifecycleRiskAuthorizer` to
validate developer mode and the secret on every install attempt. Historical
grants are audit records only and are never authorization cache. Only after
fresh authorization does the manager create/persist the commit-specific grant
and perform the use case. Secret values use redacted `Debug`, are never cloned
into errors/logs, and are dropped after authorization.

Transaction order:

```text
1. Acquire the per-plugin operation lock.
2. Validate snapshot/plugin/risk approval.
3. Persist risk audit if granted and write prepared/grant mirrors.
4. Call begin and poll status every 500 ms.
5. On built, verify artifact metadata and insert SQLite row as installing with
   install_operation_id.
6. Call finalize.
7. On finalized, mark SQLite installed and clear install_operation_id.
8. Acknowledge only after the SQLite row and finalized attestation agree.
9. On pre-SQLite failure, call abort and acknowledge after cleanup.
10. On post-SQLite finalize uncertainty, preserve installing state for startup
   reconciliation instead of deleting business state.
```

After Core attempts the first `begin` RPC, submitted prepared/grant mirrors are
owned and deleted only by Supervisor, even if the response is lost. Core may
delete its own files only when local validation fails before any RPC attempt.
On an ambiguous begin response, poll the operation ID instead of deleting
files or creating another operation. Persist and redact build logs before
mapping the public response; public errors contain only stable codes and
generic messages.

During restart reconciliation, call `plugin.install.list`, join every returned
operation with pending and installed SQLite rows, and handle both sides of the
join. A finalized operation that exactly matches an installed row's plugin ID,
repository ID, commit SHA, source/manifest hashes, and image ID is acknowledged
without removing runtime assets. A definite `failed`/`aborted` operation
causes Core to request idempotent abort cleanup and call conditional
`rollback_install(plugin_id, operation_id)` only after Supervisor confirms no
candidate or managed image remains. A built/finalized operation without a
SQLite row is aborted or removed as appropriate before acknowledgement.
Transport uncertainty preserves the operation and any `installing` row for the
next reconciliation pass.

Store per-plugin mutexes in a bounded lock registry. Remove an entry only while
holding the registry guard and only when no operation or waiter retains the
same lock; cleanup must never allow two live mutexes for one plugin ID.
Call `PluginManagerService::reconcile_install_operations()` during startup
before runtime-mode reconciliation.

- [ ] **Step 5: Run install, storage, and Supervisor client tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test install_service && cargo test -p audiodown-server --test development_config && cargo test -p audiodown-server --test plugin_install_api && cargo test -p audiodown-storage && cargo test -p audiodown-server --test supervisor_client'
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add Cargo.lock crates/audiodown-plugin-manager crates/audiodown-server
git commit -m "阶段2：安装已校验仓库插件" \
  -m "完成风险授权和可恢复的两阶段插件安装事务。"
```

### Task 13: Add Uninstall, Enablement, Priority, and Runtime Modes

**Files:**
- Modify: `crates/audiodown-plugin-manager/src/service.rs`
- Modify: `crates/audiodown-plugin-manager/src/lib.rs`
- Modify: `crates/audiodown-server/src/plugin_manager_adapters.rs`
- Modify: `crates/audiodown-server/src/routes/plugins.rs`
- Modify: `crates/audiodown-server/src/app.rs`
- Modify: `crates/audiodown-supervisor/src/server.rs`
- Modify: `crates/audiodown-supervisor/src/docker.rs`
- Test: `crates/audiodown-plugin-manager/tests/management_service.rs`
- Test: `crates/audiodown-server/tests/plugin_management_api.rs`
- Test: `crates/audiodown-supervisor/tests/remove_policy.rs`

- [ ] **Step 1: Write failing manager and management API tests**

Required routes:

```text
PATCH  /api/v1/plugins/:plugin_id
DELETE /api/v1/plugins/:plugin_id
```

PATCH body:

```json
{"enabled":true,"runMode":"on_demand","priority":100}
```

Test:

```text
priority outside 0..=1000 -> INVALID_PRIORITY
unknown runMode -> JSON rejection
manual start -> manager lock, runtime start/inspect handshake, persist healthy, touch last_used_at
manual stop -> manager lock, runtime stop, persist stopped
runtime inspect -> manager maps runtime state and never bypasses the operation lock
install racing start/stop/settings/uninstall -> PLUGIN_OPERATION_IN_PROGRESS
disable -> stop runtime first, then persist disabled state
enable -> persist installed/stopped state without starting on_demand plugin
switch to always -> start plugin, persist settings, then report healthy
switch from always to on_demand -> keep current process until idle timeout
Supervisor unavailable -> no settings mutation when a runtime action is required
uninstall -> stop/remove container, remove only matching managed image/install directory, then delete SQLite row
uninstall failure -> preserve SQLite row with last_error
unknown plugin -> PLUGIN_NOT_FOUND
```

Use fake storage and runtime adapters to prove
`PluginManagerService::update_settings` and
`PluginManagerService::start`, `stop`, `inspect_runtime`, and `uninstall` own
every state transition. Each operation must use the Task 12 per-plugin lock.
Axum tests must cover the existing lifecycle routes and assert that routes only
parse HTTP input, invoke the service, and map typed results; route modules
contain no SQLite calls, Supervisor sequencing, filesystem removal, or status
mutation.

- [ ] **Step 2: Run management tests and verify they fail**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test management_service && cargo test -p audiodown-server --test plugin_management_api && cargo test -p audiodown-supervisor --test remove_policy'
```

Expected: FAIL because the manager use cases, thin routes, and remove policy are
incomplete.

- [ ] **Step 3: Implement manager-owned settings and managed removal**

Add typed start, stop, runtime-inspect, settings, and uninstall commands to
`PluginManagerService`. Route every existing manual lifecycle endpoint through
these methods. The service acquires the per-plugin operation lock and uses
`PluginRuntimeControl`; SQLite remains the only business source of truth.
Apply ordering exactly:

```text
manual start -> start, inspect/handshake, then persist healthy and last_used_at
manual stop -> stop, then persist stopped
runtime inspect -> inspect under the same lock and persist only validated runtime status
disable -> stop successfully, then persist enabled=false
enable on_demand -> persist enabled=true without starting
switch to always -> start successfully, then persist runMode=always and runtime state
switch always to on_demand -> persist mode and retain the current process
priority-only change -> validate and persist without a runtime call
uninstall -> stop/remove managed runtime assets, then delete the SQLite row
runtime failure -> preserve prior settings/row and record a redacted last_error
```

Do not infer success from an `install.json` mirror or container presence. The
service returns the post-operation record loaded from SQLite.

Supervisor removal must independently verify installation ID, plugin ID, and
managed-image labels before deleting anything. It may remove only:

```text
the matching managed runtime container
the matching managed image
/data/plugins/installed/com.audiodown.virtual.content
```

It must reject symlinked install directories and never recursively delete from
a caller-provided path. Core returns the expanded plugin list item:

```json
{
  "pluginId": "com.audiodown.virtual.content",
  "pluginType": "content",
  "platformId": "virtual",
  "name": "Virtual Content",
  "version": "1.0.0",
  "status": "installed",
  "enabled": true,
  "runMode": "on_demand",
  "priority": 100,
  "sourceUrl": "https://github.com/example-owner/example-repository",
  "commitSha": "0123456789abcdef0123456789abcdef01234567"
}
```

- [ ] **Step 4: Run management and existing lifecycle tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test management_service && cargo test -p audiodown-server --test plugin_management_api && cargo test -p audiodown-supervisor && cargo test -p audiodown-server'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-manager crates/audiodown-server crates/audiodown-supervisor
git commit -m "阶段2：管理已安装插件设置" \
  -m "实现插件启禁、优先级、运行模式和受管卸载。"
```

### Task 14: Add Always-Run and Idle-Stop Reconciliation

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/audiodown-plugin-manager/src/service.rs`
- Modify: `crates/audiodown-plugin-manager/src/lib.rs`
- Create: `crates/audiodown-server/src/lifecycle.rs`
- Modify: `crates/audiodown-server/src/lib.rs`
- Modify: `crates/audiodown-server/src/main.rs`
- Modify: `crates/audiodown-server/src/config.rs`
- Modify: `crates/audiodown-server/src/plugin_manager_adapters.rs`
- Test: `crates/audiodown-plugin-manager/tests/lifecycle_service.rs`
- Test: `crates/audiodown-server/tests/lifecycle_reconciler.rs`

- [ ] **Step 1: Write failing reconciler tests with paused Tokio time**

Test:

```text
enabled always plugin starts during reconciliation
always plugin already healthy is not restarted
always plugin fails at most 3 consecutive automatic starts and becomes unhealthy
disabled plugin never starts
healthy on_demand plugin touched within timeout remains running
healthy on_demand plugin idle past timeout stops and becomes stopped
installed or stopped plugin is not redundantly stopped
reconciler skips a plugin while a user operation holds its lock
Supervisor unavailable leaves persisted settings intact and records a redacted error
```

Manager tests must prove automatic start/stop attempts use the same per-plugin
lock and state-transition helpers as user operations without recursively
acquiring the mutex. Server reconciler tests must use a fake manager service
and prove the scheduler contains no SQLite, route, or Supervisor client logic.

- [ ] **Step 2: Run reconciler tests and verify they fail**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test lifecycle_service && cargo test -p audiodown-server --test lifecycle_reconciler'
```

Expected: FAIL because the reconciler is missing.

- [ ] **Step 3: Implement the lifecycle loop**

Defaults:

```text
AUDIODOWN_PLUGIN_RECONCILE_SECONDS=30
AUDIODOWN_PLUGIN_IDLE_TIMEOUT_SECONDS=900
```

Values below 5 seconds are accepted only in developer mode. Touch
`last_used_at` after successful manual start and future capability calls.
Spawn one reconciler task during server startup and stop it through a
cancellation channel during graceful shutdown. Do not make Supervisor a
required dependency for serving historical APIs.

Enable Tokio's `test-util` feature for paused-time reconciler tests.
The server loop only wakes on the configured interval and invokes
`PluginManagerService::reconcile_due_plugins`. The manager selects due plugin
IDs and calls its `try_reconcile_plugin` method. That method uses `try_lock`;
it never waits behind a user operation, and it owns storage reads, runtime
calls, retry accounting, and persisted status/error changes. It skips an
`installing` plugin and never calls an Axum route internally.

Factor start/stop transitions into private helpers that require an existing
per-plugin lock guard. Public user methods acquire the lock and call those
helpers; `try_reconcile_plugin` obtains the guard with `try_lock` and calls the
same helpers directly. Never call a public locking method while already
holding that plugin's guard.

- [ ] **Step 4: Run lifecycle and workspace tests**

Run:

```bash
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test lifecycle_service && cargo test -p audiodown-server --test lifecycle_reconciler && cargo test --workspace'
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/audiodown-plugin-manager crates/audiodown-server
git commit -m "阶段2：协调插件运行模式" \
  -m "协调常驻插件启动和按需插件空闲停止。"
```

### Task 15: Execute the MCP-Driven Complete UI Redesign

**Plan:**
- Execute:
  `docs/superpowers/plans/2026-07-11-audiodown-mcp-ui-redesign-implementation-plan.md`

This approved plan revision expands Task 15 into ten ordered, independently
tested and committed UI tasks. Do not execute the superseded single-page UI
implementation. Do not create an extra umbrella commit after the ten UI
commits.

- [ ] **Step 1: Verify phase-two backend prerequisites**

Run:

```bash
git status --short --branch
docker run --rm -v "$(pwd):/workspace" -w /workspace rust:1.88-bookworm \
  sh -lc 'cargo test -p audiodown-plugin-manager --test lifecycle_service && cargo test -p audiodown-server --test lifecycle_reconciler'
```

Expected: clean worktree and PASS through phase-two Task 14.

- [ ] **Step 2: Execute UI redesign Tasks 1-10**

Follow the referenced plan exactly with MCP lookup, red-green TDD, verification,
and one commit per task.

- [ ] **Step 3: Verify the complete UI**

Run:

```bash
docker run --rm -v "$(pwd)/web:/app" -w /app node:22-bookworm-slim \
  sh -lc 'npm ci && npm test -- --run && npm run typecheck && npm run build'

docker run --rm --ipc=host -v "$(pwd)/web:/app" -w /app \
  mcr.microsoft.com/playwright:v1.61.1-noble \
  sh -lc 'npm ci && npx playwright test'
```

Expected: all unit, accessibility, responsive, and visual checks PASS.

- [ ] **Step 4: Continue to Task 16**

Confirm the latest ten commits correspond to the UI plan and continue directly
to Task 16 without an additional Task 15 commit.

### Task 16: Complete Repository Installation Integration and Acceptance

**Files:**
- Create: `test-fixtures/repositories/virtual/audiodown-repository.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-content/audiodown-plugin.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-content/package.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-content/package-lock.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-content/src/index.js`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-build-risk/audiodown-plugin.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-build-risk/package.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-build-risk/package-lock.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-build-risk/src/index.js`
- Create: `test-fixtures/github-mock/server.js`
- Create: `tests/plugin-phase2-verification-wiring.sh`
- Create: `tests/plugin-repository-smoke.sh`
- Create: `tests/plugin-installation-security.sh`
- Create: `web/tests/plugin-installation-live.spec.ts`
- Modify: `scripts/verify.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Create: `docs/phase-2-acceptance.md`

- [ ] **Step 1: Write and run a failing verification-wiring test**

Create `tests/plugin-phase2-verification-wiring.sh`. It must parse
`scripts/verify.sh` and `.github/workflows/ci.yml` and assert that both phase
two integration scripts, Vue unit/type/build checks, mocked MCP UI Playwright
checks, and the live `plugin-installation-live.spec.ts` browser check are
invoked in the required order. It also asserts failure logs/screenshots are
collected and no image-publish step exists.

Run:

```bash
./tests/plugin-phase2-verification-wiring.sh
```

Expected: FAIL because phase-two smoke/security checks are not wired into
verification or CI yet. This is the Task 16 red test; fixture creation below
must not be used as the reason for failure.

- [ ] **Step 2: Add a runnable virtual repository and mock GitHub fixture**

The repository contains only generic virtual identifiers and no real platform
name/domain. Its normal virtual content plugin must already have a complete
lockfile, build command, SDK handshake entry point, and deterministic output so
the later smoke test can exercise runtime lifecycle rather than fail on an
incomplete fixture.

It also contains one `com.audiodown.virtual.build-risk` fixture whose declared
`preinstall` script waits five seconds and writes a deterministic marker. The
latter exists only to make the build container observable and to verify
lifecycle-script grants. The mock server implements exactly:

```text
GET /repos/example-owner/example-repository
GET /repos/example-owner/example-repository/commits/main
GET /example-owner/example-repository/tar.gz/0123456789abcdef0123456789abcdef01234567
```

It rejects every other path and emits no credentials. Add a fixture self-check
inside the smoke script that validates the archive hash, manifests, lockfiles,
and mock routes before starting Core or Supervisor.

- [ ] **Step 3: Create and run the end-to-end smoke test**

The smoke test must:

```text
1. Create a temporary data directory and unique Compose project.
2. Build a deterministic tar.gz snapshot and calculate a fixed 40-char commit SHA.
3. Start Core and Supervisor with developer-only GitHub base overrides.
4. Start an ephemeral Node mock GitHub server on the Compose network.
5. POST the canonical github.com URL to repository inspection.
6. Verify preview repository ID, commit SHA, and virtual plugin.
7. Install the plugin through the public install API.
8. Verify SQLite/API source URL, commit SHA, image ID, and installed state.
9. Start and handshake the plugin.
10. Change priority and switch always -> on_demand.
11. Stop and uninstall the plugin.
12. Verify the container, managed image, installed directory, and SQLite row are gone.
13. Launch Playwright against the real Core URL without API route mocks.
14. Open Plugins and submit the canonical repository URL through the UI.
15. Verify the UI renders repository ID, locked commit, and virtual plugins.
16. Install the build-risk plugin through the UI using the explicit checkbox
    and password developer-token field.
17. Verify the real Core accepts the UI JSON body and
    x-audiodown-dev-token header by completing the granted build.
18. Enable, change mode/priority, start, stop, and uninstall through UI controls.
19. Verify API/SQLite/runtime cleanup and the UI installed list returns empty.
20. Verify Core and Supervisor remain healthy.
```

`web/tests/plugin-installation-live.spec.ts` must not call `page.route`,
`route.fulfill`, or any mock helper. It receives the real base URL and fixture
token through test-only environment variables. The token is entered into the
UI field and is never printed in Playwright output, traces, screenshots, or
failure attachments.

The smoke script runs it on the Compose network:

```bash
docker run --rm --ipc=host \
  --network "${compose_project}_default" \
  -e AUDIODOWN_LIVE_BASE_URL="http://audiodown:18080" \
  -e AUDIODOWN_LIVE_DEV_TOKEN="$fixture_dev_token" \
  -v "$(pwd)/web:/app" -w /app \
  mcr.microsoft.com/playwright:v1.61.1-noble \
  sh -lc 'npm ci && npx playwright test tests/plugin-installation-live.spec.ts'
```

The test configuration reads `AUDIODOWN_LIVE_BASE_URL` without serializing
`AUDIODOWN_LIVE_DEV_TOKEN` into config or reporters. The live spec sets:

```ts
test.use({ trace: "off", screenshot: "off", video: "off" });
```

so the password field value cannot enter Playwright artifacts.

Run it after the fixture self-check passes:

```bash
./tests/plugin-repository-smoke.sh
```

Expected: PASS. If it fails, diagnose and fix only the cross-component defect
within the Task 1-15 behavior; do not weaken the smoke assertions.

- [ ] **Step 4: Add security boundary tests**

`plugin-installation-security.sh` must prove:

```text
Core still has no Docker Socket.
Only Supervisor has the Docker Socket.
GitHub tokens/private repository inputs are rejected.
Archive traversal, symlink, oversize, and duplicate-path fixtures are rejected.
Malicious build-output traversal, links, devices, FIFOs, duplicates, and oversize entries are rejected.
Caller cannot send Docker fields through plugin.install.
Build container has no Docker Socket, Core data, downloads, Host network, or privileged mode.
Build container joins only the internal build network and can reach only the fixed proxy.
Build proxy alone joins the internal network and one operation-scoped egress bridge.
Build proxy has no host mounts or Docker Socket and keeps its fixed resource/security limits.
Runtime plugin still has network disabled and phase-one restrictions.
Lifecycle scripts do not run without a matching developer-mode grant.
Failed builds leave no prepared file, managed image, install directory, or SQLite row.
Core restart lists and reconciles unacknowledged built/finalized operations.
Operations are acknowledged only after SQLite and Supervisor state agree.
Uninstall cannot remove an image or directory with mismatched installation labels.
No automatic update request occurs after installation.
```

Every failed invariant must print a distinct `PLUGIN_INSTALL_SECURITY:` message.

For build-container assertions, start the granted build-risk installation in
the background, poll for the managed build/proxy labels while its five-second
script is running, inspect both containers and their networks, then wait for
the API request to finish. First call the same install without approval and
prove no marker, build container, image, or risk-grant mirror is created.

Run:

```bash
./tests/plugin-installation-security.sh
```

Expected: PASS. Any failure must be fixed at the owning phase-two module before
verification wiring is changed.

- [ ] **Step 5: Extend verification and CI**

Append in order:

```text
Plugin manager unit tests
Repository API tests
Supervisor build policy tests
Vue unit, typecheck, and build tests
MCP UI Playwright accessibility, responsive, and visual tests
Repository installation smoke
Live UI-to-Core repository install/manage/uninstall smoke
Plugin installation security
```

The Docker CI job preserves logs on failure and uploads:

```text
Core/Supervisor logs
mock GitHub logs
build proxy logs
plugin build logs
runtime plugin logs
Playwright report and failed UI screenshots
redacted test diagnostics
```

Do not publish images.

- [ ] **Step 6: Update verified documentation**

Document only:

```bash
docker compose up -d --build
curl http://localhost:18080/healthz
./scripts/verify.sh
```

Explain that users may enter a public GitHub repository URL in the plugin page,
that installation is locked to one commit, and that lifecycle scripts require
developer mode and explicit approval. Clearly retain these exclusions:

```text
no private repositories or GitHub tokens
no automatic plugin updates
no real platform plugins
no credentials or Cookie handling
no search/discover data
no downloads or post-processing
```

- [ ] **Step 7: Run the wiring test and full verification**

Run:

```bash
./tests/plugin-phase2-verification-wiring.sh
./scripts/verify.sh
```

Expected: all checks PASS.

- [ ] **Step 8: Commit the verified integration changes**

```bash
git add test-fixtures tests scripts web .github README.md
git commit -m "阶段2：验证安全插件安装闭环" \
  -m "验证 GitHub 仓库快照安装、受限 Node 构建、插件管理和安全边界。"
```

- [ ] **Step 9: Run acceptance from a clean clone**

Run:

```bash
rm -rf /tmp/audiodown-core-phase2-review
git clone . /tmp/audiodown-core-phase2-review
cd /tmp/audiodown-core-phase2-review
./scripts/verify.sh
```

Expected: PASS without untracked files or external repositories.

- [ ] **Step 10: Record acceptance evidence**

`docs/phase-2-acceptance.md` records:

```text
tested base commit
Docker/Compose/Rust/Node versions
full verification result
repository inspect and locked commit evidence
install/start/settings/stop/uninstall lifecycle
live browser-to-Core repository install and developer-token header flow
managed image and install directory cleanup
build proxy and runtime security assertions
known phase-two exclusions
```

Do not include tokens, complete host paths, Docker Socket metadata, full
environment dumps, or complete GitHub response headers.

- [ ] **Step 11: Amend the Task 16 commit with acceptance evidence**

```bash
git add docs/phase-2-acceptance.md
git commit --amend --no-edit
```

- [ ] **Step 12: Push and require CI success**

```bash
git push origin main
gh run list --branch main --limit 3
run_id="$(gh run list --branch main --limit 1 --json databaseId --jq '.[0].databaseId')"
gh run watch "$run_id" --exit-status --interval 10
```

Expected: remote `main` equals local `HEAD`, unit/static and Docker integration
jobs pass, and no managed test container remains.

## Phase-Two Definition of Done

The plan is complete only when all statements are true:

- A user can inspect a public GitHub repository from the plugin page.
- Core resolves the default branch to one immutable commit before downloading.
- Archive and repository limits reject traversal, links, oversize content, and malformed indexes.
- Plugin manifests, compatibility ranges, entries, package manifests, lockfiles, resolved URLs, and integrity fields are validated.
- Core stages immutable metadata but cannot choose arbitrary Docker image, command, mount, network, or resource policy.
- Supervisor independently verifies source and manifest hashes before building.
- Node builds use the fixed Dockerfile and SDK.
- Untrusted build containers have no direct egress and can reach npm only through the exact-host CONNECT proxy.
- Lifecycle scripts are ignored by default and require a commit-specific developer-mode risk grant.
- Successful installs persist source URL, repository ID, commit SHA, source hash, manifest hash, image ID, settings, and status.
- Plugins can be enabled, disabled, prioritized, switched between on-demand and always modes, started, stopped, idled, and uninstalled.
- Failed installs and uninstalls preserve transactional consistency and do not leave unmanaged resources.
- Runtime plugin isolation remains at least as strict as phase one.
- The plugin UI exposes installation and management without hardcoded real platforms.
- No private GitHub access, GitHub token, automatic update, credentials, Cookie Jar, real search/discover data, download, archive, or post-processing behavior exists.
- `./scripts/verify.sh` passes from a clean clone.
- GitHub `main` contains every phase-two commit and CI passes.

## Follow-Up Plan Boundaries

Do not add these while executing this plan:

- Content search, discover, album, track, cursor, aggregation, fallback, or deduplication RPC.
- Runtime HTTP proxy requests to content hosts.
- Credential plugin UI, QR flows, manual Cookie import, Cookie Jars, token storage, or encryption.
- Download plan/resolve, tasks, transfer, progress, retries, or files.
- Private GitHub repositories, GitHub tokens, branch following, polling, or automatic updates.
- Real platform names, domains, plugins, migrations, or reverse-engineered request logic.
- Archive organization, format conversion, HLS/DASH, post-processing, or old-data import.

Those belong to plans 3-6 and depend on this installation boundary remaining stable.
