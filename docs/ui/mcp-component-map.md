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
