# MCP Component Map

| AudioDown surface | MCP reference | Vue implementation |
| --- | --- | --- |
| Desktop shell | sidebar-07 | shadcn-vue Sidebar |
| Mobile navigation | sheet | shadcn-vue Sheet |
| Data views | data-table-demo | semantic Vue Table |
| Empty states | empty-demo | shadcn-vue Empty |
| Forms | field-demo | shadcn-vue Field |
| Destructive actions | alert-dialog-demo | shadcn-vue AlertDialog |
| Loading | skeleton-demo | shadcn-vue Skeleton |

React/TSX source is not copied. All runtime components are Vue/Reka UI.

## Shared Component Adaptations

- `AsyncState` combines `skeleton-demo` and `alert` while leaving data fetching
  to each view.
- `EmptyState` keeps the `empty-demo` hierarchy and limits commands to one
  primary and one optional secondary action.
- `ResponsiveDialog` selects the Vue/Reka Dialog above 760px and the
  `vaul-vue` Drawer at or below 760px, with one shared content contract.
- `vue-sonner` is mounted once beside the root application component; inline
  errors remain owned by the affected view.

## Application Shell Adaptations

- `sidebar-07` supplies the desktop icon-rail behavior; AudioDown keeps only
  product identity, five primary routes, empty-core status, and the collapse
  command.
- `dashboard-01` supplies the 52px sticky header and fluid workspace spacing;
  redundant single-segment breadcrumbs and all sample dashboard data are
  omitted.
- Mobile navigation uses the generated Vue/Reka Sheet and closes after route
  selection.
- Desktop collapse state is stored only as
  `audiodown.sidebar.collapsed`; the generated sidebar Cookie persistence is
  disabled.

## Content Empty-State Adaptations

- Discover uses one unframed `empty-demo` composition with a Compass icon and
  the Core-provided title and action label.
- Search keeps the Field/Input/Button query group outside `AsyncState`, so
  loading and retry never replace or clear the query.
- Skeleton and Alert replace text-only loading and error lines. No sample
  content, platform, chart, or repository data is introduced.

## Repository Installation Adaptations

- `field-demo` supplies labeled URL and developer-token fields; the token is
  password-only, memory-only, and sent exclusively in
  `x-audiodown-dev-token`.
- Dialog and Drawer share one URL/preview/installing state machine through
  `ResponsiveDialog`; lifecycle risk approval is an inline preview section,
  never a nested modal.
- Repository and plugin metadata use Badge plus selectable rows. Inspection
  remains available when Supervisor is unavailable, while installation is
  disabled.
- Skeleton reports inspection/build progress, Alert keeps failures inline,
  and Sonner reports only successful completion without sensitive values.

## Installed Plugin Management Adaptations

- `data-table-demo` is reduced to a semantic desktop table because installed
  plugin counts do not justify TanStack sorting, pagination, selection, or
  column controls.
- The same plugin data becomes a stable, `min-width: 0` item layout below the
  desktop breakpoint; mobile management does not use a horizontally scrolling
  table.
- Switch updates enabled state optimistically and rolls back on failure.
  Runtime commands, settings, errors, and busy state remain scoped to one
  plugin row.
- Run mode and priority use a right-side Sheet. Start and stop remain labeled
  icon commands with Tooltip text, while settings and uninstall live in a
  compact DropdownMenu.
- Uninstall uses `alert-dialog-demo`, names the selected plugin, and remains
  disabled with other runtime-changing commands when Supervisor is
  unavailable.

## Structured Log Adaptations

- `data-table-demo` becomes a semantic four-column desktop table and a
  non-scrolling mobile item list. Rows open one details Sheet and preserve the
  original ISO timestamp in `datetime` and the detail view.
- Level, component, and text filters are local to the latest list returned by
  Core. The toolbar does not add backend query parameters, polling, export, or
  deletion commands.
- Manual refresh keeps active filters. A failed refresh leaves the previous
  successful list visible and adds an inline Alert.
- The initial request uses stable table-shaped Skeleton rows; a genuinely
  empty response uses a compact `empty-demo` composition.

## System Status Adaptations

- The system page uses one full-width semantic definition list with four
  stable rows for Core version, Supervisor availability, installed plugin
  count, and developer mode.
- Loading keeps the same four-row dimensions with Skeleton values, so the
  first viewport does not shift after `/system` resolves.
- StatusBadge combines icon and text. Developer mode is always warning-toned,
  while disabled developer mode remains neutral.
- Supervisor unavailability and developer mode share one Alert. Low-level
  Supervisor error details are not rendered, and the page has no restart,
  update, or secret-related controls.
