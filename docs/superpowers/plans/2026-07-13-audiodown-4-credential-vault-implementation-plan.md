# AudioDown 1.0 Credential Vault Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the phase-four credential loop: typed credential plugin contracts, AES-256-GCM encrypted storage, secure local master-key bootstrap, temporary Core-owned Cookie Jar sessions, a policy-enforced Core HTTP proxy, virtual QR and manual-import flows, credential-aware proxy requests, redacted logs, and a usable credentials UI.

**Architecture:** `audiodown-domain` owns stable credential scope and status models. `audiodown-plugin-api` owns the six credential RPC methods and declarative QR/status results. A new `audiodown-credential-vault` crate owns encryption, key loading, credential lifecycle, and temporary login flows. A new `audiodown-network-proxy` crate owns host policy, DNS/IP validation, redirect revalidation, filtered headers, bounded bodies, Cookie Jars, and credential injection. Core exposes a private authenticated proxy backend on a dedicated Unix Socket that is never mounted into plugin containers. Supervisor creates one internal Docker network per running plugin and starts a fixed AudioDown gateway sidecar on that network; the sidecar alone mounts the proxy Socket and relays bounded authenticated requests. The plugin can connect only to that gateway, receives a fresh in-memory proxy token through the authenticated Supervisor control plane, and never shares a network with Core Web, other plugins, or public egress. Axum remains a thin adapter and Vue renders only Core-defined declarative models.

**Tech Stack:** Rust 1.88, Axum, Tokio, SQLx/SQLite, serde, AES-256-GCM, OS randomness, Unix file permissions, authenticated Unix sockets, reqwest/rustls, bounded HTTP streaming, Node.js 22 SDK, Vue 3, TypeScript, Vitest, Playwright, Docker Compose.

---

## Delivery Roadmap

1. Foundation and virtual plugin lifecycle - complete.
2. GitHub repository installation and secure Node builds - complete.
3. Content capabilities and search/discovery aggregation - complete.
4. **Credential vault, credential plugins, and scoped HTTP proxying - this plan.**
5. Task engine and Core downloader.
6. Hardening, migration interfaces, diagnostics, and release.

This plan does not add real platform names, real platform domains, real login, automatic Cookie acquisition, private repositories, download planning, file downloads, archive organization, format conversion, Core automatic updates, plugin-provided Dockerfiles, or non-Node runtimes.

## Progress Snapshot

As of 2026-07-13, phases one through three are complete, `./scripts/verify.sh` passes, and `main` is synchronized with GitHub. Phase four has no implementation yet. Existing content behavior and all phase-one through phase-three security boundaries are a required non-regression baseline.

## Locked Decisions

- The only phase-four credential plugin methods are `credential.qr.start`, `credential.qr.poll`, `credential.import`, `credential.status`, `credential.refresh`, and `credential.logout`. `credential.import` is only a post-Core-import status notification carrying credential identity and scope; Cookie plaintext never enters this RPC.
- `CredentialScope` is a bounded lowercase ASCII dotted identifier. Credential plugins declare provided scopes. Content plugins declare optional or required scopes. Core validates declarations against plugin type and platform.
- A manifest scope declaration is never an authorization grant. Every encrypted credential record persists its normalized exact target origin set independently of the source plugin. Before installation or first use, the user must explicitly grant each content plugin access to selected declared optional/required scopes and exact origins from the intersection of the content declaration and the credential record. Grants are persisted per plugin ID, installed manifest hash, credential ID, scope, and origin set; a manifest or credential-origin change invalidates them. Users can inspect and revoke grants without uninstalling the plugin.
- QR results are declarative data only: payload, optional display code, expiry, poll interval, and safe status. Plugins cannot return HTML, JavaScript, Vue components, remote scripts, or executable UI.
- Core creates an opaque login-flow ID and a separate opaque Cookie Jar Session ID. Both are random, bounded, short-lived, bound to one plugin and one scope, and kept in memory only.
- `Set-Cookie` is consumed by the Core Cookie Jar and removed from the response returned to plugins. Plugins receive only the Cookie Jar Session ID.
- A credential plugin may request promotion only for a scope it provides and only from the flow-bound temporary Cookie Jar. Core performs promotion, encryption, persistence, and logging.
- Manual Cookie import requires the user to select one exact origin declared for the credential scope. Core parses the input into host-only, `Secure`, `Path=/` Cookie records bound to that origin and encrypts them. Cookie plaintext never enters plugin RPC parameters, SQLite plaintext columns, structured logs, tracing logs, browser artifacts, or test artifacts. The plugin receives only scope and credential identity for a status check.
- Credential secrets are versioned structured values encrypted with AES-256-GCM. Every encryption uses a fresh 96-bit nonce. Associated data binds record ID, scope, algorithm version, and key version.
- The master key is a 32-byte OS-random file under `data/credentials/`, created atomically with directory mode `0700` and file mode `0600`. Symlinks, non-regular files, wrong length, and unsafe permissions fail closed.
- SQLite stores ciphertext, nonce, scope, normalized credential target origins, credential kind, source plugin, platform, key version, expiry, status metadata, scope grants, and timestamps. It never stores Cookie or Token plaintext.
- Vault APIs expose metadata directly but release plaintext only to trusted Core services through a narrow secret callback/guard. No public route or plugin RPC can export plaintext.
- The Core HTTP proxy accepts only `http` for an explicit developer-mode virtual fixture mapping; production requests require `https`.
- Host permission comes from the installed manifest, never from a caller-provided allowlist. Exact hosts and one-label wildcard suffixes are supported.
- DNS answers are resolved and validated before connection. Loopback, private, link-local, multicast, unspecified, documentation-only, and cloud metadata destinations are blocked. The validated address is pinned for the request.
- Redirects are disabled in the HTTP client and followed manually. Every hop repeats scheme, host, DNS/IP, header, credential-scope, and size checks. Redirect count is bounded.
- Plugin-controlled headers cannot set `Host`, `Cookie`, `Set-Cookie`, `Authorization`, proxy headers, forwarding headers, connection headers, or transfer framing. Core owns credential injection.
- Request headers, request body, response headers, decompressed response body, redirect count, timeout, and concurrent proxy calls have fixed limits.
- Credential injection requires all of: the installed manifest declares the scope, an unrevoked user grant matches the current manifest hash and credential ID, the requested origin is in the intersection of the content declaration, credential target origins, and grant, the stored credential scope/platform matches, and the request host is allowed by the manifest. Content plugins cannot retrieve decrypted values from any Core API; Core injects them only into the outbound request and blocks direct secret reflections in headers and bodies.
- Plugins access only a fixed AudioDown HTTP Gateway sidecar on a Supervisor-created per-plugin internal network. The gateway alone receives the dedicated proxy Socket volume read-only; plugin containers never mount any Core or proxy Socket.
- Every plugin runtime generation receives one fresh 256-bit proxy token that remains valid only for that generation. Core stores the token only in memory, Supervisor passes it only through the authenticated control request and container environment, and all Debug/log output redacts it. Stop, failed start, replacement start, uninstall, and Core restart revoke it; stale tokens from earlier generations are never renewed or accepted.
- All plugins remain without public or private egress and have no public ports, no Core data mount, no downloads mount, no Docker Socket, no Host network, no Host PID, no privileged mode, a read-only root filesystem, dropped capabilities, and fixed resource limits. Their only network peer is the fixed gateway sidecar.
- A developer-only exact virtual-host mapping may target the local virtual credential service during tests. Configuration is rejected unless developer mode is enabled and never changes production private-address blocking.
- Credential operations and proxy requests enter the unified log with Core-owned plugin, version, platform, request, flow, scope, result, duration, and error fields. URLs are reduced to scheme/host/path class; query values, headers, bodies, Cookie values, tokens, QR payloads, and secrets are never logged.
- Uninstalling a credential plugin with stored credentials requires an explicit retain/delete choice. Retained credentials clear the source-plugin ownership but remain usable by authorized content plugins through the proxy.
- The UI uses the existing MCP-selected shadcn/Reka system. It adds a work-focused Credentials route with declarative QR, manual import, status, refresh, logout, and explicit uninstall disposition. It does not render plugin-supplied markup.
- Phase four keeps the public Web UI local-only by publishing `127.0.0.1:18080:18080`. Credential mutation routes require same-origin `Origin`, exact configured `Host`, `application/json`, no CORS, bounded rate limits, and `Cache-Control: no-store`. Remote management and multi-user authentication remain outside phase four.
- An authorized content plugin receives delegated account-use capability for the granted scope and origins, but never a plaintext-export capability. The proxy blocks direct secret disclosure in headers/bodies and logs; user review and revocation of grants are the trust boundary for account actions. A remote service that colludes with a plugin to transform or encode an injected secret is outside the phase-four threat model because a generic HTTP response cannot prove semantic non-disclosure.
- The encrypted envelope and internal proxy ports support both Cookie and Token credentials. Phase four exposes only Core-owned manual Cookie import and synthetic Cookie login flows. Token plaintext may enter only through a trusted internal Core port, never plugin RPC or public HTTP; unit and security fixtures exercise Token encryption, injection, and redaction without adding a real Token acquisition flow.

## Stable Public HTTP Shape

```text
GET    /api/v1/credentials
POST   /api/v1/credential-flows/qr
POST   /api/v1/credential-flows/{flowId}/poll
DELETE /api/v1/credential-flows/{flowId}
POST   /api/v1/credentials/import
POST   /api/v1/credentials/{credentialId}/status
POST   /api/v1/credentials/{credentialId}/refresh
POST   /api/v1/credentials/{credentialId}/logout
GET    /api/v1/credential-grants
POST   /api/v1/credential-grants
DELETE /api/v1/credential-grants/{grantId}
DELETE /api/v1/plugins/{pluginId}?credentials=retain|delete
```

Public credential responses contain identity, credential kind, plugin/platform/scope, normalized target origins, safe status, expiry, timestamps, and safe errors. They never contain ciphertext, nonce, key version, Cookie values, Token values, proxy tokens, Cookie Jar Session IDs, or plugin opaque state.

## Internal Gateway Shape

The Node SDK sends one bounded JSON request to the fixed gateway URL:

```json
{
  "token": "<ephemeral>",
  "requestId": "<bounded>",
  "method": "GET",
  "url": "https://allowed.example/path",
  "headers": {},
  "bodyBase64": null,
  "cookieJarSessionId": null,
  "credentialScope": null
}
```

The gateway forwards the request to Core's dedicated Unix Socket and has no Core Web, data, downloads, control-token, Docker Socket, or public-network access. Core derives plugin identity, manifest hosts, persisted grants, credential target origins, and declared scopes from the token registry and SQLite. The response contains only status, filtered headers, bounded body, and a safe standard error. `Set-Cookie`, directly reflected injected request credentials, and sensitive response headers are never returned.

## Execution Rules

- Start every Task with `git status --short --branch`.
- Execute Tasks in order without merging, skipping, or expanding them.
- For every Task: write the failing test, run it and observe the intended failure, implement the minimum code, run the stated passing verification, then commit.
- Use the exact suggested Chinese commit subject and add one concise Chinese commit body.
- Diagnose failures. Do not delete tests, weaken limits, bypass private-address checks, expose secrets for convenience, or accept an unrelated failing baseline.
- Do not begin Task 1 unless phase-three `main` is synchronized with GitHub, `./scripts/verify.sh` passes, and the latest GitHub CI run succeeds.
- Prefer local Rust/Cargo when available; otherwise use the repository Docker verification pattern with `rust:1.88-bookworm`.
- Before any test that handles credential plaintext, disable Playwright screenshots, traces, and video for that test and use unique canaries that are asserted absent from artifacts and logs.
- Do not push intermediate phase-four commits. Push `main` only after Task 20 passes from a clean checkout.

### Task 1: Define Credential Domain Models

**Files:**
- Create: `crates/audiodown-domain/src/credential.rs`
- Modify: `crates/audiodown-domain/src/lib.rs`
- Create: `crates/audiodown-domain/tests/credential.rs`

- [ ] **Step 1: Write failing domain tests**

Cover bounded `CredentialScope`, credential IDs, source-plugin ownership, active/expired/revoked/error states, safe public metadata, and rejection of uppercase, whitespace, path separators, URLs, empty segments, overlong values, and more than the allowed segment count.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-domain --test credential
```

Expected: FAIL because credential domain types do not exist.

- [ ] **Step 3: Implement the minimum domain model**

Add dependency-free scope, status, metadata, and identity types. Keep encryption, SQLite, HTTP, Docker, Cookie parsing, and plugin RPC out of the domain crate.

- [ ] **Step 4: Run domain checks**

```bash
cargo test -p audiodown-domain
cargo clippy -p audiodown-domain --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-domain
git commit -m "阶段4：定义凭据领域模型" \
  -m "增加受限凭据作用域、状态和安全公共元数据。"
```

### Task 2: Define Credential Plugin RPC Contracts

**Files:**
- Create: `crates/audiodown-plugin-api/src/credential.rs`
- Modify: `crates/audiodown-plugin-api/src/error.rs`
- Modify: `crates/audiodown-plugin-api/src/lib.rs`
- Modify: `crates/audiodown-plugin-api/src/manifest.rs`
- Create: `crates/audiodown-plugin-api/tests/credential_contracts.rs`
- Modify: `crates/audiodown-plugin-api/tests/content_contracts.rs`

- [ ] **Step 1: Write failing contract tests**

Cover strict serde round trips and size validation for all six methods, QR presentation, pending/scanned/confirmed/expired states, safe account status, promotion requests, bounded plugin opaque state, and the standard credential errors. Assert that `credential.import` accepts only credential identity and scope after Core has encrypted the imported Cookie:

```text
INVALID_REQUEST
PLUGIN_NOT_FOUND
PLUGIN_DISABLED
PLUGIN_CAPABILITY_MISSING
PLUGIN_UNAVAILABLE
PLUGIN_TIMEOUT
PLUGIN_RESPONSE_INVALID
CREDENTIAL_NOT_FOUND
CREDENTIAL_EXPIRED
CREDENTIAL_SCOPE_NOT_ALLOWED
LOGIN_FLOW_NOT_FOUND
LOGIN_FLOW_EXPIRED
LOGIN_PENDING
LOGIN_DENIED
RATE_LIMITED
PLATFORM_RESPONSE_CHANGED
PLUGIN_INTERNAL_ERROR
```

Assert that no request/result field can contain Cookie plaintext, arbitrary headers, proxy tokens, HTML, scripts, or caller-controlled Docker/runtime values.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-plugin-api --test credential_contracts
```

Expected: FAIL because credential RPC contracts do not exist.

- [ ] **Step 3: Implement the minimum contract**

Add `CredentialMethod`, strict request/result structs, declarative QR data, polling/status models, promotion intent, bounded opaque plugin state, and validation helpers. Do not add network transport or vault implementation.

- [ ] **Step 4: Run plugin API checks**

```bash
cargo test -p audiodown-plugin-api
cargo clippy -p audiodown-plugin-api --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-api
git commit -m "阶段4：定义凭据插件协议" \
  -m "定义六类凭据调用、声明式二维码和安全状态契约。"
```

### Task 3: Validate Credential Scope Declarations

**Files:**
- Modify: `crates/audiodown-plugin-api/src/manifest.rs`
- Modify: `crates/audiodown-plugin-api/tests/contracts.rs`
- Modify: `crates/audiodown-plugin-api/tests/repository_contracts.rs`
- Modify: `crates/audiodown-plugin-manager/src/validation.rs`
- Modify: `crates/audiodown-plugin-manager/tests/repository_validation.rs`
- Modify: `crates/audiodown-plugin-manager/src/staging.rs`
- Modify: `web/src/api/client.ts`
- Modify: `web/src/components/plugins/RepositoryPreviewDialog.vue`
- Modify: related plugin preview tests

- [ ] **Step 1: Write failing manifest and preview tests**

Require credential plugins to declare one or more `providedScopes`; allow content plugins to declare bounded `requiredScopes` and `optionalScopes`; require every scope to declare exact target origins; reject duplicates, malformed scopes/origins, cross-platform scopes, provided scopes on content plugins, requested scopes on credential plugins, and undeclared credential capabilities. Require repository preview to show provided/requested scopes and origins as security-relevant capabilities, collect an explicit grant decision for each requested scope, and make clear that declaration alone does not authorize use.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-plugin-api --test contracts --test repository_contracts
cargo test -p audiodown-plugin-manager --test repository_validation
cd web && npm test -- --run src/components/plugins
```

Expected: FAIL because manifests have no credential declarations and credential capabilities remain unsupported.

- [ ] **Step 3: Implement minimum manifest support**

Add a strict optional `credentials` block, enforce plugin-type, platform, and exact-origin invariants, add the six methods to the credential capability allowlist, preserve phase-three content validation, and expose only safe declaration metadata plus explicit grant choices to the preview. Do not persist grants yet.

- [ ] **Step 4: Run validation checks**

```bash
cargo test -p audiodown-plugin-api
cargo test -p audiodown-plugin-manager --test repository_validation
cd web && npm test -- --run
cd web && npm run typecheck
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-plugin-api crates/audiodown-plugin-manager web
git commit -m "阶段4：校验凭据作用域声明" \
  -m "按插件类型和虚拟平台校验提供与请求的凭据作用域。"
```

### Task 4: Extend the Node SDK for Credential Plugins

**Files:**
- Create: `plugin-sdk/node/src/credential.js`
- Create: `plugin-sdk/node/src/proxy.js`
- Modify: `plugin-sdk/node/src/index.js`
- Modify: `plugin-sdk/node/src/rpc.js`
- Create: `plugin-sdk/node/test/credential.test.js`
- Create: `plugin-sdk/node/test/proxy.test.js`
- Modify: `plugin-sdk/node/test/rpc.test.js`

- [ ] **Step 1: Write failing Node tests**

Test six method constants, typed handler wrapping, strict request/result validation, safe plugin errors, unexpected-error redaction, declarative QR limits, fixed HTTP Gateway framing, bounded messages, timeout handling, token redaction, and rejection of Cookie/Authorization/Set-Cookie access through helper return values.

- [ ] **Step 2: Run and confirm failure**

```bash
cd plugin-sdk/node && npm test
```

Expected: FAIL because credential and proxy helpers do not exist.

- [ ] **Step 3: Add minimum SDK helpers**

Add `createCredentialHandlers` and a Core proxy client using only the fixed Supervisor-provided `AUDIODOWN_PROXY_URL` and `AUDIODOWN_PROXY_TOKEN`. The URL must be an exact internal `http` origin with no caller-controlled path, query, credentials, or alternate host. Keep handler registration limited to built-in system, phase-three content, and phase-four credential methods. The SDK must not log or expose the token.

- [ ] **Step 4: Run SDK checks**

```bash
cd plugin-sdk/node && npm test
```

- [ ] **Step 5: Commit**

```bash
git add plugin-sdk/node
git commit -m "阶段4：扩展凭据插件 SDK" \
  -m "提供受限凭据处理器和固定内部 Gateway 客户端。"
```

### Task 5: Implement the AES-256-GCM Envelope

**Files:**
- Create: `crates/audiodown-credential-vault/Cargo.toml`
- Create: `crates/audiodown-credential-vault/src/lib.rs`
- Create: `crates/audiodown-credential-vault/src/crypto.rs`
- Create: `crates/audiodown-credential-vault/tests/crypto.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing crypto tests**

Cover round-trip encryption, fresh nonce uniqueness, wrong-key failure, tampered ciphertext/nonce/AAD failure, bounded plaintext, version rejection, cross-record/scope/algorithm-version/key-version envelope substitution, and redacted Debug output. Use test canaries and assert they never appear in formatted errors.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-credential-vault --test crypto
```

Expected: FAIL because the vault crate and encrypted envelope do not exist.

- [ ] **Step 3: Implement minimum encryption**

Use AES-256-GCM with OS randomness, a 96-bit nonce, versioned envelope, and canonical associated data binding record ID, scope, algorithm version, and key version. Keep secrets in secrecy wrappers and return stable non-sensitive errors.

- [ ] **Step 4: Run crypto checks**

```bash
cargo test -p audiodown-credential-vault
cargo clippy -p audiodown-credential-vault --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/audiodown-credential-vault
git commit -m "阶段4：实现凭据加密封装" \
  -m "使用随机 Nonce 和绑定元数据的 AES-256-GCM 保存秘密。"
```

### Task 6: Bootstrap the Local Master Key Securely

**Files:**
- Create: `crates/audiodown-credential-vault/src/key_store.rs`
- Create: `crates/audiodown-credential-vault/tests/key_store.rs`
- Modify: `crates/audiodown-credential-vault/src/lib.rs`
- Modify: `crates/audiodown-server/src/config.rs`
- Modify: `crates/audiodown-server/src/main.rs`
- Modify: `.gitignore`

- [ ] **Step 1: Write failing key-store and configuration tests**

Require atomic first-start creation, exact 32-byte reuse, directory `0700`, file `0600`, rejection of symlinks/non-regular files/wrong length/unsafe permissions, no key logging, and `data/credentials/` bootstrap. Confirm test helpers never create a key inside the repository.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-credential-vault --test key_store
cargo test -p audiodown-server --test development_config
```

Expected: FAIL because no key store or credential directory configuration exists.

- [ ] **Step 3: Implement minimum key bootstrap**

Create the credentials directory and master key using create-new semantics, restrictive Unix permissions, fsync, and fail-closed validation. Add a configurable key path derived from `AUDIODOWN_DATA_DIR`; do not add an environment variable containing key material.

- [ ] **Step 4: Run bootstrap checks**

```bash
cargo test -p audiodown-credential-vault
cargo test -p audiodown-server
cargo clippy -p audiodown-credential-vault -p audiodown-server --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add .gitignore crates/audiodown-credential-vault crates/audiodown-server
git commit -m "阶段4：安全生成本机主密钥" \
  -m "以原子写入和严格权限管理本机凭据主密钥。"
```

### Task 7: Persist Encrypted Credential Records

**Files:**
- Create: `migrations/0004_credentials.sql`
- Create: `crates/audiodown-storage/src/credential_repository.rs`
- Modify: `crates/audiodown-storage/src/lib.rs`
- Create: `crates/audiodown-storage/tests/credentials.rs`
- Modify: `crates/audiodown-storage/tests/storage.rs`

- [ ] **Step 1: Write failing migration and repository tests**

Cover fresh migration and upgrade from phase three, insert/upsert/list/get/delete, unique scope ownership, ciphertext/nonce/key-version persistence, credential kind, normalized exact target origins, safe metadata, retained credential ownership clearing, expiry, and proof that schema columns and rows contain no plaintext canary. Cover per-content-plugin scope grants bound to plugin ID, manifest hash, credential ID, scope, and exact origin set; require every grant origin to be in the credential-origin intersection, explicit creation, revocation, manifest/credential-origin invalidation, and uninstall cleanup.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-storage --test credentials --test storage
```

Expected: FAIL because migration `0004` and the repository do not exist.

- [ ] **Step 3: Implement minimum persistence**

Add encrypted credential storage with credential kind and normalized target origins, scope-grant storage, and indexes. Keep temporary Cookie Jars, runtime proxy tokens, and login flow state out of SQLite. Decode malformed stored values as stable storage errors without printing ciphertext.

- [ ] **Step 4: Run storage checks**

```bash
cargo test -p audiodown-storage
cargo clippy -p audiodown-storage --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add migrations/0004_credentials.sql crates/audiodown-storage
git commit -m "阶段4：持久化加密凭据记录" \
  -m "在 SQLite 中保存密文、安全元数据和显式作用域授权。"
```

### Task 8: Build the Credential Vault Service

**Files:**
- Create: `crates/audiodown-credential-vault/src/service.rs`
- Create: `crates/audiodown-credential-vault/src/secret.rs`
- Modify: `crates/audiodown-credential-vault/src/lib.rs`
- Modify: `crates/audiodown-credential-vault/Cargo.toml`
- Create: `crates/audiodown-credential-vault/tests/service.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing vault service tests**

Cover encrypted Cookie and Token create/update through trusted Core ports, normalized target-origin binding, metadata-only list/status, narrow secret access, expiry, logout/delete, retained-source clearing, concurrent updates, optimistic conflict handling, rollback on failure, and absence of plaintext from errors and Debug output. Assert public and plugin-facing ports cannot create or export Token plaintext. Master-key rotation is not part of phase four because the single-file key design has no crash-safe versioned keyring.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-credential-vault --test service
```

Expected: FAIL because no vault lifecycle service exists.

- [ ] **Step 3: Implement minimum vault lifecycle**

Add a repository port, versioned structured Cookie and Token secret payloads, normalized target origins, encrypted upsert, metadata status, trusted internal create port, internal secret callback/guard, delete, ownership clearing, and conflict-safe update. Do not add a public Token import or plugin Token acquisition path, and do not expose plaintext through serde or public HTTP types.

- [ ] **Step 4: Run vault checks**

```bash
cargo test -p audiodown-credential-vault
cargo clippy -p audiodown-credential-vault --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/audiodown-credential-vault
git commit -m "阶段4：实现凭据金库服务" \
  -m "完成密文生命周期、状态查询和受限秘密访问。"
```

### Task 9: Enforce Proxy Host and Address Policy

**Files:**
- Create: `crates/audiodown-network-proxy/Cargo.toml`
- Create: `crates/audiodown-network-proxy/src/lib.rs`
- Create: `crates/audiodown-network-proxy/src/policy.rs`
- Create: `crates/audiodown-network-proxy/src/resolver.rs`
- Create: `crates/audiodown-network-proxy/tests/policy.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing policy tests**

Cover exact and wildcard host matching, HTTPS-only production URLs, ports, credentials in URLs, fragments, IP literals, loopback/private/link-local/multicast/unspecified/documentation/cloud-metadata addresses, mixed DNS answers, DNS Rebinding between checks, redirect host changes, and developer-only exact fixture mapping rejection outside developer mode.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-network-proxy --test policy
```

Expected: FAIL because the network proxy crate and address policy do not exist.

- [ ] **Step 3: Implement minimum policy engine**

Add injected DNS resolution, global-address classification, validated address pinning, strict URL parsing, manifest host matching, and developer-only fixture mapping. Do not perform HTTP or handle Cookie data yet.

- [ ] **Step 4: Run policy checks**

```bash
cargo test -p audiodown-network-proxy
cargo clippy -p audiodown-network-proxy --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/audiodown-network-proxy
git commit -m "阶段4：实现代理地址安全策略" \
  -m "阻断私网地址、DNS Rebinding 和未授权主机。"
```

### Task 10: Implement Bounded HTTP Proxy Transport

**Files:**
- Create: `crates/audiodown-network-proxy/src/http.rs`
- Create: `crates/audiodown-network-proxy/src/error.rs`
- Modify: `crates/audiodown-network-proxy/src/lib.rs`
- Modify: `crates/audiodown-network-proxy/Cargo.toml`
- Create: `crates/audiodown-network-proxy/tests/http.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing HTTP transport tests**

Use an injected local transport/resolver to cover pinned DNS addresses, manual redirect revalidation, redirect loops, method restrictions, forbidden request headers, filtered response headers, request/response/header limits, decompressed-body limits, timeout, cancellation, concurrency limits, invalid encoding, and stable standard errors.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-network-proxy --test http
```

Expected: FAIL because no bounded HTTP transport exists.

- [ ] **Step 3: Implement minimum transport**

Add fixed limits, manual redirects, per-hop policy checks, safe header allowlists, streamed bounded bodies, fixed timeouts, and error mapping. Do not inject credentials or retain Set-Cookie yet.

- [ ] **Step 4: Run transport checks**

```bash
cargo test -p audiodown-network-proxy
cargo clippy -p audiodown-network-proxy --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/audiodown-network-proxy
git commit -m "阶段4：实现受限 HTTP 代理" \
  -m "固定请求边界并在每次重定向重新执行安全校验。"
```

### Task 11: Add Temporary Cookie Jars and Scoped Injection

**Files:**
- Create: `crates/audiodown-network-proxy/src/cookie_jar.rs`
- Create: `crates/audiodown-network-proxy/src/credential.rs`
- Create: `crates/audiodown-network-proxy/src/service.rs`
- Modify: `crates/audiodown-network-proxy/src/http.rs`
- Modify: `crates/audiodown-network-proxy/src/lib.rs`
- Modify: `crates/audiodown-network-proxy/Cargo.toml`
- Create: `crates/audiodown-network-proxy/tests/cookie_jar.rs`
- Create: `crates/audiodown-network-proxy/tests/credential_proxy.rs`
- Modify: `Cargo.lock`

- [ ] **Step 1: Write failing Cookie and credential tests**

Cover random flow-bound jar sessions, expiry, host/path/secure matching, multiple Set-Cookie headers, deletion/expiry attributes, rejection of public-suffix or unrelated-domain cookies, stripping Set-Cookie from plugin responses, promotion snapshots with normalized credential origins, and a separate refresh Jar that can atomically replace a stored credential after successful validation. Credential injection must require manifest declaration, current-manifest user grant bound to credential ID, exact origin intersection, matching platform/scope, and a non-expired credential; test revoked/missing/stale grants, changed credential origins, undeclared scopes, wrong origins, expired credentials, redirect re-evaluation, direct Cookie/Token reflection rejection, Token authorization injection through a trusted synthetic fixture, and redacted logs.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-network-proxy --test cookie_jar --test credential_proxy
```

Expected: FAIL because Cookie Jars and vault-backed injection do not exist.

- [ ] **Step 3: Implement minimum Cookie and credential handling**

Add in-memory TTL login/refresh Jars, RFC-oriented Cookie selection, Core-owned Cookie or Token authorization headers, a narrow vault secret port, persisted-grant and credential-origin authorization, promotion snapshots, atomic refresh replacement, direct-secret response rejection, and redacted proxy log events. Plugins receive only jar IDs and filtered responses.

- [ ] **Step 4: Run proxy checks**

```bash
cargo test -p audiodown-network-proxy
cargo clippy -p audiodown-network-proxy --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.lock crates/audiodown-network-proxy
git commit -m "阶段4：实现临时 Cookie Jar" \
  -m "由 Core 管理登录与刷新 Cookie，并按显式授权注入请求。"
```

### Task 12: Expose an Authenticated Core Proxy Backend

**Files:**
- Create: `crates/audiodown-server/src/proxy_gateway.rs`
- Create: `crates/audiodown-server/src/proxy_adapters.rs`
- Modify: `crates/audiodown-server/src/lib.rs`
- Modify: `crates/audiodown-server/src/state.rs`
- Modify: `crates/audiodown-server/src/config.rs`
- Modify: `crates/audiodown-server/src/main.rs`
- Modify: `crates/audiodown-server/Cargo.toml`
- Create: `crates/audiodown-server/tests/proxy_gateway.rs`
- Modify: `crates/audiodown-network-proxy/src/cookie_jar.rs`
- Modify: `Cargo.lock`

Design correction: the Node SDK sends the opaque Cookie Jar session ID over the
bounded JSON gateway protocol, while Task 11 intentionally kept the UUID field
private and omitted a parser. Task 12 therefore adds only a canonical public
UUID parser to `CookieJarSessionId`; Jar creation and ownership remain in Core.

- [x] **Step 1: Write failing gateway tests**

Cover 256-bit token generation, runtime-generation-to-plugin binding, registration/revocation, one bounded JSON request per Unix connection, wrong/missing/stale/revoked token rejection, claimed plugin spoofing rejection, manifest-derived hosts plus persisted scope grants and credential origins, concurrency, idle timeout, malformed/oversized messages, socket cleanup, and redacted token Debug/log output. The backend must expose no HTTP route and accept no request that bypasses the token registry.

- [x] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-server --test proxy_gateway
```

Expected: FAIL because Core has no proxy gateway or token registry.

- [x] **Step 3: Implement minimum gateway**

Start a Unix listener at the configured dedicated proxy path, register runtime-generation-bound tokens in memory, derive plugin policy from SQLite installation state, current grants, and credential origins, call `audiodown-network-proxy`, and revoke all tokens on shutdown or runtime replacement. The Socket directory contains no other Core files and is intended only for the fixed Gateway sidecar added in Task 13.

- [x] **Step 4: Run gateway checks**

```bash
cargo test -p audiodown-server --test proxy_gateway
cargo test -p audiodown-server
cargo clippy -p audiodown-server --all-targets -- -D warnings
```

- [x] **Step 5: Commit**

```bash
git add Cargo.lock crates/audiodown-server crates/audiodown-network-proxy/src/cookie_jar.rs docs/superpowers/plans/2026-07-13-audiodown-4-credential-vault-implementation-plan.md
git commit -m "阶段4：开放受认证代理后端" \
  -m "使用临时运行令牌将代理请求绑定到可信插件身份。"
```

### Task 13: Isolate Plugins Behind the Fixed Gateway

**Files:**
- Create: `crates/audiodown-proxy-gateway/Cargo.toml`
- Create: `crates/audiodown-proxy-gateway/src/main.rs`
- Create: `crates/audiodown-proxy-gateway/tests/relay.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create: `docker/plugin-gateway.Dockerfile`
- Modify: `docker-compose.yml`
- Modify: `crates/audiodown-supervisor-protocol/src/lib.rs`
- Modify: `crates/audiodown-supervisor-protocol/tests/contracts.rs`
- Modify: `crates/audiodown-server/src/supervisor.rs`
- Modify: `crates/audiodown-server/src/plugin_manager_adapters.rs`
- Modify: `crates/audiodown-server/tests/supervisor_client.rs`
- Modify: `crates/audiodown-supervisor/src/config.rs`
- Modify: `crates/audiodown-supervisor/src/policy.rs`
- Modify: `crates/audiodown-supervisor/src/docker.rs`
- Modify: `crates/audiodown-supervisor/tests/policy.rs`
- Modify: `crates/audiodown-supervisor/tests/protocol.rs`
- Modify: `tests/security-boundary.sh`

- [ ] **Step 1: Write failing protocol and runtime-policy tests**

Require the fixed Gateway binary to relay one bounded request between an internal HTTP listener and the Core Unix Socket without logging bodies or tokens. Require trusted start requests to carry only plugin ID and a redacted proxy token. Supervisor must derive a per-plugin internal network, fixed Gateway image/name/alias, fixed backend Socket path, and configured proxy volume; the Gateway alone receives the read-only proxy volume, while the plugin receives only the exact Gateway URL and token. Reject caller-controlled paths, images, commands, volume names, mounts, networks, aliases, or tokens from public HTTP. Extend security assertions to prove the plugin network contains only that plugin and its Gateway, has `internal=true`, has no public/private egress, and cannot reach Core Web or another plugin.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-proxy-gateway
cargo test -p audiodown-supervisor-protocol
cargo test -p audiodown-server --test supervisor_client
cargo test -p audiodown-supervisor --test policy --test protocol
./tests/security-boundary.sh
```

Expected: FAIL because the fixed Gateway image, per-plugin internal network, and derived runtime policy do not exist.

- [ ] **Step 3: Implement minimum runtime integration**

Add an explicit Compose proxy volume with a configurable installation-scoped name and mount it only into Core. Build the fixed Gateway image from repository-owned code. On plugin start, register the token, create the per-plugin internal network, start the Gateway with the proxy volume mounted read-only, attach only the plugin and Gateway, and pass the exact Gateway URL/token to the plugin. On start failure, stop, remove, uninstall, or Core restart, revoke the token and remove the Gateway/network. Retain all existing sandbox settings for both containers; neither receives Core data, downloads, control token, Docker Socket, public ports, or an external network.

- [ ] **Step 4: Run runtime checks**

```bash
cargo test -p audiodown-proxy-gateway
cargo test -p audiodown-supervisor-protocol
cargo test -p audiodown-server --test supervisor_client
cargo test -p audiodown-supervisor
cargo clippy -p audiodown-proxy-gateway --all-targets -- -D warnings
./tests/security-boundary.sh
docker compose config
```

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock docker-compose.yml docker/plugin-gateway.Dockerfile crates/audiodown-proxy-gateway crates/audiodown-supervisor-protocol crates/audiodown-server crates/audiodown-supervisor tests/security-boundary.sh
git commit -m "阶段4：隔离插件代理运行通道" \
  -m "使用每插件内部网络和固定 Gateway 隔离代理访问。"
```

### Task 14: Invoke Credential Plugins through the Supervisor

**Files:**
- Modify: `crates/audiodown-supervisor-protocol/src/lib.rs`
- Modify: `crates/audiodown-supervisor-protocol/tests/contracts.rs`
- Modify: `crates/audiodown-server/src/supervisor.rs`
- Modify: `crates/audiodown-supervisor/src/docker.rs`
- Modify: `crates/audiodown-supervisor/tests/content_rpc.rs`
- Modify: `crates/audiodown-supervisor/tests/protocol.rs`
- Modify: `crates/audiodown-plugin-manager/src/service.rs`
- Create: `crates/audiodown-plugin-manager/tests/credential_invocation.rs`
- Modify: `crates/audiodown-server/src/plugin_manager_adapters.rs`
- Modify: related fake runtime tests

- [ ] **Step 1: Write failing typed invocation tests**

Require the Supervisor method enum to allow exactly phase-three content and phase-four credential methods, validate method-specific params, preserve fixed timeout/size/identity checks, reject arbitrary strings, and use the managed container only. Require manager invocation to validate credential plugin type, enabled state, capability, scope declaration, active-call lease, on-demand start, touch time, safe errors, and structured logs.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-supervisor-protocol
cargo test -p audiodown-supervisor --test content_rpc --test protocol
cargo test -p audiodown-plugin-manager --test credential_invocation
```

Expected: FAIL because plugin RPC is content-only and manager has no credential invocation path.

- [ ] **Step 3: Implement minimum credential invocation**

Introduce a typed plugin-method wrapper, generalize the fixed Unix Socket exec bridge without accepting arbitrary methods, and add `invoke_credential` with the existing lifecycle and lease guarantees. Keep raw plugin errors out of public results and logs.

- [ ] **Step 4: Run invocation checks**

```bash
cargo test -p audiodown-supervisor-protocol
cargo test -p audiodown-supervisor
cargo test -p audiodown-plugin-manager
cargo clippy -p audiodown-supervisor-protocol -p audiodown-supervisor -p audiodown-plugin-manager --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-supervisor-protocol crates/audiodown-supervisor crates/audiodown-plugin-manager crates/audiodown-server
git commit -m "阶段4：接入凭据插件调用" \
  -m "通过受限 Supervisor RPC 启动并调用六类凭据能力。"
```

### Task 15: Orchestrate Credential Login and Lifecycle Flows

**Files:**
- Create: `crates/audiodown-server/src/credential_service.rs`
- Create: `crates/audiodown-server/src/credential_adapters.rs`
- Modify: `crates/audiodown-server/src/state.rs`
- Modify: `crates/audiodown-server/src/main.rs`
- Create: `crates/audiodown-server/tests/credential_service.rs`

- [ ] **Step 1: Write failing service tests**

Cover QR start, poll cadence, pending/scanned/confirmed/expired/denied states, flow cancellation, jar and exact-origin binding, promotion only after plugin confirmation, scope/origin spoofing rejection, flow expiry, duplicate polling, manual-import exact-origin status check without plaintext RPC, host-only Secure Path=/ Cookie construction, status, refresh through a separate Core-owned refresh Jar, atomic credential replacement only after successful refresh validation, refresh rollback, logout local/remote failure policy, plugin failure, Core restart invalidation, and safe logs for all six methods.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-server --test credential_service
```

Expected: FAIL because no Core credential orchestration service exists.

- [ ] **Step 3: Implement minimum orchestration**

Add an in-memory bounded flow store, bind each flow and jar to one exact declared origin before QR start, call credential plugins through the manager, promote only the flow-bound jar with that origin, persist through the vault, and expose metadata-only service results. Manual import validates one exact declared origin, constructs host-only Secure Path=/ Cookie records, encrypts in Core first, and passes only scope and credential ID to `credential.import`. Refresh creates a short-lived Core-owned Jar seeded from the stored credential and origins, captures replacement Cookie state, and commits it atomically only after the plugin reports success. A remote refresh/logout failure retains the last known local credential and records a safe error; confirmed logout revokes the local record.

- [ ] **Step 4: Run service checks**

```bash
cargo test -p audiodown-server --test credential_service
cargo test -p audiodown-server
cargo clippy -p audiodown-server --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add crates/audiodown-server
git commit -m "阶段4：编排凭据登录流程" \
  -m "由 Core 管理二维码轮询、Cookie Jar 提升和凭据生命周期。"
```

### Task 16: Expose Credential HTTP APIs and Manual Import

**Files:**
- Modify: `docker-compose.yml`
- Create: `crates/audiodown-server/src/routes/credentials.rs`
- Modify: `crates/audiodown-server/src/routes/mod.rs`
- Modify: `crates/audiodown-server/src/app.rs`
- Create: `crates/audiodown-server/tests/credential_api.rs`
- Modify: `crates/audiodown-logging/src/redaction.rs`
- Modify: `crates/audiodown-logging/tests/redaction.rs`

- [ ] **Step 1: Write failing API and redaction tests**

Cover strict request bodies, unknown fields, IDs/scopes/size limits, metadata-only list including credential origins and kind, QR start/poll/cancel, manual Cookie import with one exact declared origin and host-only Secure Path=/ normalization, scope-grant list/create/revoke bound to credential ID and origin intersection, status/refresh/logout, stable HTTP error mapping, request IDs, no-cache headers, bounded per-route rate limits, and full redaction of Cookie, Set-Cookie, Authorization, Token, proxy token, jar ID, QR payload, ciphertext, nonce, and master-key canaries. Mutation tests must reject a missing/wrong `Origin`, wrong `Host`, non-JSON content type, CORS preflight from another origin, and requests sent through a non-loopback host publish.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-server --test credential_api
cargo test -p audiodown-logging
```

Expected: FAIL because routes and complete credential redaction do not exist.

- [ ] **Step 3: Implement minimum HTTP surface**

Publish the Web UI as `127.0.0.1:18080:18080`, add thin Axum adapters and Core-owned Cookie parsing, and enforce exact configured Host, same-origin mutations, JSON-only bodies, no CORS, and bounded route rate limits. Add metadata-only grant endpoints; creating a grant must match the installed manifest hash, credential ID, declared scope, and the intersection of declared and credential exact origins. Keep Token creation internal-only. Never serialize internal vault envelopes or flow secrets. Apply `Cache-Control: no-store` to credential responses and redact before writing structured/tracing logs.

- [ ] **Step 4: Run API checks**

```bash
cargo test -p audiodown-server --test credential_api
cargo test -p audiodown-logging
cargo test -p audiodown-server
cargo clippy -p audiodown-server -p audiodown-logging --all-targets -- -D warnings
```

- [ ] **Step 5: Commit**

```bash
git add docker-compose.yml crates/audiodown-server crates/audiodown-logging
git commit -m "阶段4：开放凭据管理接口" \
  -m "提供声明式登录和 Core 标准手工 Cookie 导入接口。"
```

### Task 17: Require Explicit Credential Disposition on Uninstall

**Files:**
- Create: `migrations/0005_credential_deletion.sql`
- Modify: `crates/audiodown-server/src/routes/plugins.rs`
- Modify: `crates/audiodown-server/tests/plugin_management_api.rs`
- Modify: `crates/audiodown-server/tests/credential_api.rs`
- Modify: `crates/audiodown-storage/src/credential_repository.rs`
- Modify: `crates/audiodown-storage/tests/credentials.rs`
- Modify: `web/src/api/client.ts`
- Modify: `web/src/components/plugins/UninstallPluginDialog.vue`
- Modify: related Vue tests

- [ ] **Step 1: Write failing uninstall tests**

Require retain/delete only for credential plugins with stored scopes, reject missing/invalid disposition, preserve existing content-plugin uninstall behavior, revoke content-plugin grants on uninstall, clear source ownership when retaining, and show an explicit accessible UI choice. For delete, persist a `pending_delete` transition before runtime removal; roll it back if removal fails; after successful removal delete the records, and if final cleanup fails clear source ownership, retain `pending_delete`, and make retry idempotent.

- [ ] **Step 2: Run and confirm failure**

```bash
cargo test -p audiodown-server --test plugin_management_api --test credential_api
cargo test -p audiodown-storage --test credentials
cd web && npm test -- --run src/components/plugins
```

Expected: FAIL because uninstall has no credential disposition.

- [ ] **Step 3: Implement minimum disposition flow**

Add a forward-only migration for the explicit pending-delete state, then add a strict query/body choice, retained ownership clearing, grant revocation, retry-safe delete logging, and UI controls. Do not silently delete or retain credentials and never leave a credential owned by an absent plugin.

- [ ] **Step 4: Run uninstall checks**

```bash
cargo test -p audiodown-server --test plugin_management_api --test credential_api
cargo test -p audiodown-storage
cd web && npm test -- --run
cd web && npm run typecheck
```

- [ ] **Step 5: Commit**

```bash
git add migrations/0005_credential_deletion.sql crates/audiodown-server crates/audiodown-storage web
git commit -m "阶段4：明确卸载凭据处置" \
  -m "卸载凭据插件时要求用户选择保留或删除加密凭据。"
```

### Task 18: Add the Virtual Credential Plugin and Local Service

**Files:**
- Modify: `test-fixtures/repositories/virtual/audiodown-repository.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-credential/audiodown-plugin.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-credential/package.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-credential/package-lock.json`
- Create: `test-fixtures/repositories/virtual/plugins/virtual-credential/src/index.js`
- Create: `test-fixtures/services/virtual-credential-service.mjs`
- Modify: `test-fixtures/repositories/virtual/README.md`
- Create: `tests/virtual-credential-contract.sh`
- Modify: related repository fixture tests

- [ ] **Step 1: Write failing fixture contract tests**

Require one dependency-free Node 22 credential plugin with only virtual names, `virtual.web` scope, six methods, proxy-only QR/status/refresh/logout behavior, deterministic pending/scanned/confirmed transitions, promotion request, safe failures, and no direct network or real domain. Require the local service to set synthetic Cookies and return synthetic account state.

- [ ] **Step 2: Run and confirm failure**

```bash
./tests/virtual-credential-contract.sh
cargo test -p audiodown-plugin-manager --test repository_validation
```

Expected: FAIL because the virtual credential plugin and service do not exist.

- [ ] **Step 3: Implement minimum virtual loop**

Add the fixture plugin and local service. The plugin must use the SDK proxy client, reference only jar/session/scope identifiers, and never see Set-Cookie or Cookie plaintext. Keep all values synthetic and deterministic.

- [ ] **Step 4: Run fixture checks**

```bash
./tests/virtual-credential-contract.sh
cargo test -p audiodown-plugin-manager --test repository_validation
cd plugin-sdk/node && npm test
```

- [ ] **Step 5: Commit**

```bash
git add test-fixtures tests/virtual-credential-contract.sh
git commit -m "阶段4：添加虚拟凭据插件" \
  -m "提供本地虚拟二维码、状态和 Cookie Jar 契约夹具。"
```

### Task 19: Implement the Credentials UI

**Files:**
- Modify: `web/src/router.ts`
- Modify: `web/src/api/client.ts`
- Modify: the existing shell navigation component
- Create: `web/src/views/CredentialsView.vue`
- Create: `web/src/components/credentials/CredentialStatusTable.vue`
- Create: `web/src/components/credentials/QrLoginDialog.vue`
- Create: `web/src/components/credentials/CookieImportDialog.vue`
- Create: `web/src/views/CredentialsView.test.ts`
- Modify: `web/tests/fixtures/mock-api.ts`
- Modify: `web/tests/accessibility.spec.ts`
- Modify: `web/tests/ui-shell.spec.ts`
- Modify: `web/tests/visual/ui-visual.spec.ts`
- Add visual baselines under: `web/tests/visual/ui-visual.spec.ts-snapshots/`

- [ ] **Step 1: Review MCP component guidance and write failing UI tests**

Use the available shadcn MCP service to review table, dialog, tabs/segmented state, alert, input, button, progress/status, and responsive navigation patterns. Write tests for no-credential empty state, plugin/scope/credential selection, review/create/revoke scope grants with credential-ID binding, exact origin intersection, and manifest/credential-origin invalidation, declarative QR rendering, poll states, cancellation, manual Cookie import with exact-origin selection and password-style non-persistent input, status, refresh, logout, retained credentials, error summaries, keyboard operation, focus restoration, reduced motion, mobile overflow, and accessibility.

- [ ] **Step 2: Run and confirm failure**

```bash
cd web && npm test -- --run src/views/CredentialsView.test.ts
npx playwright test tests/accessibility.spec.ts tests/ui-shell.spec.ts tests/visual/ui-visual.spec.ts --grep "Credential"
```

Expected: FAIL because the credentials route and components do not exist.

- [ ] **Step 3: Implement the work-focused credential UI**

Adapt MCP guidance to the existing Vue/Reka system. Use icons for commands, dialogs for QR/import/logout, compact status rows, explicit grant/revoke controls, safe error alerts, and stable responsive dimensions. Grant UI must distinguish manifest declaration from user authorization, identify the selected credential without exposing internal secrets, and show the exact intersected origins receiving delegated account-use capability. Never echo imported Cookie text, internal flow/jar IDs, ciphertext, or plugin-supplied markup.

- [ ] **Step 4: Run UI checks**

```bash
cd web && npm test -- --run
cd web && npm run typecheck
cd web && npm run build
npx playwright test tests/accessibility.spec.ts tests/ui-shell.spec.ts tests/visual/ui-visual.spec.ts
```

- [ ] **Step 5: Commit**

```bash
git add web
git commit -m "阶段4：实现凭据管理界面" \
  -m "增加声明式二维码、手工导入、状态刷新和退出流程。"
```

### Task 20: Verify the Phase-Four End-to-End Loop

**Files:**
- Create: `tests/credential-flow-smoke.sh`
- Create: `tests/credential-security.sh`
- Create: `tests/credential-phase4-verification-wiring.sh`
- Create: `web/tests/credential-flow-live.spec.ts`
- Modify: `scripts/verify.sh`
- Modify: `.github/workflows/ci.yml`
- Create: `docs/phase-4-acceptance.md`
- Modify: `README.md`

- [ ] **Step 1: Write failing verification wiring and live tests**

Require `verify.sh` and CI to run the credential contract, virtual plugin contract, live flow, and security matrix after phase-three checks. The live loop must install the virtual credential plugin, start QR login bound to an exact virtual origin, make proxy requests through a temporary Cookie Jar, poll to confirmation, promote and encrypt the credential with its origin, verify SQLite has no plaintext, explicitly grant a virtual content plugin the declared scope, credential ID, and exact intersected virtual origin, use that scope through the fixed Gateway, revoke and confirm denial, re-grant, refresh through a separate Jar, logout, manually import a synthetic Cookie with an exact origin through Core, inspect redacted logs, stop the plugin, and test retain/delete uninstall choices.

Security checks must prove:

- credential methods outside the six-method allowlist are rejected;
- each plugin has a unique `internal=true` network containing only itself and the fixed Gateway; plugins have no proxy Socket mount, public/private egress, Core Web access, or route to another plugin;
- only the fixed Gateway mounts the dedicated proxy Socket volume, and it has no Core data, downloads, control token, Docker Socket, public ports, or external network;
- content and credential plugins cannot directly read Core-stored Cookie or Token plaintext, ciphertext, nonce, master key, proxy token of another plugin, Core data, downloads, Core control token, proxy Socket, or Docker Socket; a trusted internal synthetic Token fixture must prove encryption, injection, direct-reflection rejection, and redaction without a public Token import route;
- manifest host permissions, declared scopes, persisted user grants, manifest-hash binding, credential-ID binding, credential target origins, exact origin intersection, and grant revocation cannot be bypassed;
- loopback, private, link-local, cloud metadata, mixed DNS, DNS Rebinding, and redirects to blocked addresses fail;
- forbidden headers, oversized messages/bodies, redirect loops, timeouts, malformed Set-Cookie, expired jars, expired credentials, and spoofed flow/scope/plugin IDs fail safely;
- `Set-Cookie` is absent from plugin-visible responses;
- manual-import plaintext and all secret canaries are absent from HTTP responses, SQLite plaintext, Core logs, Supervisor logs, plugin logs, Playwright artifacts, and CI artifacts;
- credential mutations reject cross-origin, wrong-Host, non-JSON, CORS, and rate-limit bypass attempts, and host port `18080` is bound only to `127.0.0.1`;
- Core is still the only Compose service exposing `18080` and Supervisor is still the only Docker Socket owner.

- [ ] **Step 2: Run and confirm failure**

```bash
./tests/credential-phase4-verification-wiring.sh
./tests/credential-flow-smoke.sh
./tests/credential-security.sh
```

Expected: FAIL until scripts, live test, CI wiring, and all security assertions exist.

- [ ] **Step 3: Complete verification and acceptance documentation**

Wire phase-four checks after phase-three verification, update the final banner, document the encrypted vault and proxy boundaries, and update README progress to mark phase four complete. Do not claim real platform login or real Cookie acquisition.

- [ ] **Step 4: Run the complete local verification**

```bash
git status --short --branch
./scripts/verify.sh
```

Expected: the complete phase-one through phase-four suite passes in the working tree, no credential canary appears in artifacts, Compose is cleaned up, and no managed plugin containers, proxy tokens, grants, Gateway sidecars, test volumes, or test networks remain.

- [ ] **Step 5: Commit**

```bash
git add tests web/tests scripts/verify.sh .github/workflows/ci.yml docs/phase-4-acceptance.md README.md
git commit -m "阶段4：验证凭据金库完整闭环" \
  -m "接入加密凭据、代理隔离、实时界面、安全矩阵和阶段验收。"
```

- [ ] **Step 6: Verify the committed phase from a clean checkout**

```bash
verification_dir="$(mktemp -d /tmp/audiodown-phase4.XXXXXX)"
git clone --no-local . "$verification_dir/repo"
cd "$verification_dir/repo"
./scripts/verify.sh
```

Expected: the clone contains the Task 20 commit and the complete phase-one through phase-four suite passes. If this committed-tree verification fails, fix the root cause, rerun Step 4, amend the Task 20 commit, and repeat Step 6 before any push.

## Phase Completion

After Task 20:

```bash
git status --short --branch
./scripts/verify.sh
git push origin main
```

Confirm:

- `main` is clean and synchronized with `origin/main`;
- GitHub CI succeeds for the pushed commit;
- phases one through three remain green;
- the local master key is generated safely and never enters Git, SQLite, or logs;
- SQLite stores only encrypted credential payloads and safe metadata;
- virtual QR and manual import flows complete without passing plaintext through plugins;
- temporary Cookie Jars remain Core-owned and `Set-Cookie` is hidden from plugins;
- the Core proxy enforces manifest hosts, persisted user grants, exact origins, manifest-hash binding, scope authorization, private-address blocking, DNS pinning, redirect revalidation, header filtering, timeouts, and size limits;
- virtual content plugins can use an authorized credential scope only through Core proxy injection;
- plugin containers remain isolated on unique internal networks, can reach only their fixed Gateway sidecar, and receive no proxy Socket mount;
- credentials UI supports declarative QR, grant/revoke review, status, refresh, logout, manual import, and explicit uninstall disposition;
- all credential and proxy logs are structured and redacted;
- no real platform, real domain, real Cookie acquisition, real download, archive, updater, private repository, plugin Dockerfile, or non-Node runtime was added.
