# UI Redesign Acceptance

## Tested Revision

- Base commit: `7dba346adb865f91b2f110508bc086f79bdb9519`
- Acceptance scope: the Task 10 visual tests, baselines, documentation, and
  mobile control hardening stored with this document
- Test date: 2026-07-12

## MCP References

- `sidebar-07` and `dashboard-01` for the responsive operational shell
- `sheet`, `dialog`, and `drawer` for mobile navigation and focused workflows
- `data-table-demo` for semantic plugin and log tables
- `empty-demo`, `skeleton-demo`, and `alert` for asynchronous states
- `field-demo`, `select`, `switch`, and `input` for repository and settings
  forms
- `alert-dialog-demo`, `dropdown-menu`, and `tooltip` for plugin commands
- `badge-demo`, `item-demo`, and `separator` for compact status presentation

React registry source was used only as a design reference. Runtime
implementations are Vue components backed by Reka UI.

## Pinned UI Versions

| Package | Version |
| --- | --- |
| Vue | 3.5.39 |
| Reka UI | 2.10.1 |
| vaul-vue | 0.4.1 |
| @lucide/vue | 1.24.0 |
| Tailwind CSS | 4.3.2 |
| @playwright/test | 1.61.1 |
| @axe-core/playwright | 4.12.1 |

The generated component configuration uses the shadcn-vue `reka-nova` style
with Lucide icons.

## Viewport Results

| Viewport | Coverage | Result |
| --- | --- | --- |
| 1440x900 | expanded/collapsed shell, tables, dialogs, sheets, system states | PASS |
| 1024x768 | navigation, repository dialog, long identifiers | PASS |
| 390x844 | mobile sheet, focus trap, 40px controls, long log wrapping | PASS |

Fifteen deterministic screenshots were reviewed. They contain no clipped or
overlapping text, nested cards, oversized headings, gradients, decorative
blobs, horizontal page overflow, or unstable toolbar/table dimensions.

## Verification Results

- Vitest: 9 files, 45 tests passed
- `vue-tsc --noEmit`: passed
- Vite production build: passed
- Axe: five routes at desktop, tablet, and mobile with no serious or critical
  violations
- Responsive and keyboard checks: 7 tests passed
- Visual baselines: 15 screenshots passed in the pinned Playwright image

## Phase-Two Workflows

- Public repository inspection and Commit SHA preview
- Normal repository preview and lifecycle-script risk authorization
- Installed plugin enable, runtime, settings, and named uninstall controls
- Structured log local filtering, refresh, responsive rows, and details
- Safe Core, Supervisor, plugin-count, and developer-mode status display

## Known Exclusions

- No real platform plugin, platform domain, or platform API
- No private repository or plugin-provided Dockerfile
- No credential acquisition, Cookie handling, download, task, or update UI
- No polling, log export, log deletion, fake content, or fake system controls

## MCP Audit

- Vue components used instead of React registry code
- Semantic component map complete
- Accessible labels and focus behavior verified
- Responsive screenshots reviewed
- No plan-excluded product behavior added
