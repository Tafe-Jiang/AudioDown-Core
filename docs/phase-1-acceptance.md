# AudioDown Core Phase-One Acceptance

## Tested Revision

- Commit: `69b5a0470fa5dbbb74be50ace22e619dff5b0b04`
- Verification date: 2026-07-11
- Verification command: `./scripts/verify.sh`
- Result: passed from a clean clone

## Toolchain

- Docker Engine: `29.5.2`
- Docker Compose: `v5.1.4`
- Rust image: `rust:1.88-bookworm`
- Rust compiler: `rustc 1.88.0`
- Node image: `node:22-bookworm-slim`
- Node runtime: `v22.23.1`

## Health Evidence

Core health:

```json
{"ok":true,"service":"audiodown-core"}
```

System response with Supervisor health:

```json
{"version":"1.0.0-alpha.1","supervisor":{"available":true,"error":null},"pluginCount":0}
```

Compose started exactly the Core and Supervisor as user-managed services. Only Core
published port `18080`.

## Lifecycle Evidence

The virtual fixture completed this lifecycle:

```text
installed -> starting -> healthy -> stopped
```

Supervisor validated the install record and manifest digest, created the restricted
container, completed `system.hello` and `system.health`, returned runtime state, exposed
attributed plugin logs through Core, and stopped the container.

## Security Evidence

The automated boundary test confirmed:

- Core had no Docker Socket mount.
- Supervisor was the only service with the Docker Socket mount.
- The virtual plugin had no public ports and no privileged mode.
- The virtual plugin used a read-only root filesystem and restricted `/tmp` tmpfs.
- The virtual plugin dropped all Linux capabilities and enabled
  `no-new-privileges`.
- Memory, CPU, and PID limits were present.
- Plugin networking was disabled and direct public DNS and connection attempts failed.
- The plugin could not read the Core SQLite database.
- The plugin had no Core data, downloads, or Docker Socket mount.

## Absence Review

Repository scans of implementation, tests, scripts, migrations, and CI found no:

- activation-code routes or models, license heartbeat, or device binding;
- real content platform names or domains;
- Cookie parsing or persistence implementation;
- GitHub automatic update behavior;
- archive organization or post-processing implementation.

## Phase-One Exclusions

This phase intentionally excludes real platform plugins, credentials, Cookie storage,
search and discovery data, real downloads, repository plugin installation, automatic
updates, archive organization, post-processing, activation, licensing, and device
binding. The virtual plugin exists only to verify contracts and isolation.
