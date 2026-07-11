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
