# AudioDown Core Phase-Two Acceptance

## Tested Revision

- Base commit: `1856fd023d05ced1ce6a95cf335ef13a107954d1`
- Verification date: 2026-07-12
- Verification command: `./scripts/verify.sh`
- Result: passed from a clean clone

## Toolchain

- Docker Engine client/server: `29.5.2`
- Docker Compose: `v5.1.4`
- Rust compiler: `rustc 1.88.0`
- Host Node runtime: `v24.16.0`
- Fixed plugin runtime image: `node:22-bookworm-slim`
- npm: `11.13.0`

## Full Verification

The clean-clone verification passed:

- Rust workspace formatting, unit, integration, documentation, and Clippy checks
- Node plugin SDK tests
- Vue unit tests, type checking, and production build
- MCP-selected UI accessibility, responsive, keyboard, and visual checks
- Compose Core and Supervisor smoke test
- Virtual plugin lifecycle and phase-one isolation checks
- Repository installation and live browser-to-Core smoke test
- Plugin installation security matrix

The final verification result was:

```text
AudioDown phase-two verification passed
```

## Repository Inspection Evidence

The synthetic public repository fixture was inspected through the public Core
API. Core resolved its default branch before downloading and locked the
snapshot to:

```text
0123456789abcdef0123456789abcdef01234567
```

The preview returned the expected repository ID and both virtual plugins.
Installation records retained the source repository, immutable Commit SHA,
source hash, manifest hash, lockfile hash, image ID, and explicit risk grant.

## Lifecycle Evidence

The dependency-free virtual plugin completed:

```text
inspect -> install -> start -> handshake -> healthy
-> settings update -> stop -> uninstall
```

The settings check changed plugin priority and switched its runtime mode from
`always` to `on_demand`. After uninstall, the API and SQLite row were absent,
the runtime container was gone, and the managed image and install directory
were removed.

## Live Browser Evidence

Playwright used the real Core UI and did not mock API routes. The browser:

- submitted the synthetic public repository address;
- displayed the repository ID, locked Commit SHA, and virtual plugins;
- explicitly approved the declared lifecycle-script risk;
- entered the developer token in a password field;
- sent the installation request body and `x-audiodown-dev-token` header to
  Core;
- completed install, enable, settings, start, stop, and uninstall actions;
- returned the installed plugin list to its empty state.

Tracing, screenshots, and video were disabled for the live test so the
developer token could not enter test artifacts.

## Security Evidence

The automated security matrix confirmed:

- Core had no Docker Socket mount; Supervisor remained the only Socket owner.
- Repository input rejected credentials, private repository forms, unsafe
  redirects, traversal, links, duplicate paths, and oversized archives.
- Package validation rejected Git, local-path, arbitrary URL, unsafe resolved
  dependency, incompatible runtime, and undeclared lifecycle-script inputs.
- Callers could not provide Docker image, command, mount, network, or resource
  fields through the install API.
- The build container had no Docker Socket, Core data, downloads, privileged
  mode, Host network, or direct egress.
- The build container reached only the fixed proxy on an internal network.
- The build proxy had no host mounts or Docker Socket and retained fixed
  security and resource limits.
- Runtime plugin networking remained disabled with the phase-one filesystem,
  capability, process, and resource restrictions.
- Lifecycle scripts did not execute without a matching commit-specific
  developer-mode grant.
- Failed and aborted builds left no prepared file, managed image, install
  directory, or SQLite installation row.
- Restart reconciliation and operation acknowledgement preserved agreement
  between SQLite and Supervisor state.
- Uninstall rejected mismatched installation identity and image labels.
- Installation triggered no automatic update request.

## Phase-Two Exclusions

Phase two intentionally contains no private repository support, GitHub token
storage, automatic plugin update, plugin-provided Dockerfile, non-Node runtime,
real platform plugin, credentials, Cookie handling, content search or
discovery data, downloads, archive organization, format conversion, or
post-processing.
