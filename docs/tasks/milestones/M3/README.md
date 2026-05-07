# M3 Subtask Index

M3 (the "main milestone") is too large to deliver in a single session. It is
split into the six self-contained slices below. Each slice is intended to land
as one PR and to be picked up by a fresh agent session.

The parent design document is
[../M3-environment-and-resolver.md](../M3-environment-and-resolver.md). Read
it before any slice — it specifies the shared architecture, the
`@__librbs_handle` ivar contract, the canonical-dump invariants, and the
acceptance criteria that the slices collectively satisfy.

| Slice | Title | Depends on |
|---|---|---|
| [M3a](M3a-resolver-foundations.md) | Resolver / UseMap / Resolution side-table (pure Rust) | M2 |
| [M3b](M3b-resolution-driver.md) | AST traversal driver + Rust-side canonical dump | M3a |
| [M3c](M3c-native-build-and-dump.md) | `build_environment` / `canonical_dump` magnus bridge + Ruby-side `canonical_dump` helper + core compat spec | M3b |
| [M3d](M3d-resolve-type-names-native.md) | `resolve_type_names` magnus bridge + `resolve-type-names: false` handling | M3c |
| [M3e](M3e-materialization.md) | AST → `RBS::AST::*` materialization (`materialize_all`) | M3d |
| [M3f](M3f-patches-and-compat.md) | Patch layer + core+stdlib + major-gems compat matrix | M3e |

## Acceptance mapping

The six acceptance checkboxes in the parent M3 doc are satisfied as follows:

| Acceptance item (parent) | Slice that closes it |
|---|---|
| All `cargo test -p librbs-core` tests are green | M3a, M3b |
| Canonical dumps for core only match pure RBS exactly | M3c (after Ruby-side helper exists) and verified again at M3f |
| Canonical dumps for core + stdlib match pure RBS exactly | M3f |
| Major-gems matrix is green | M3f |
| `from_loader` / `resolve_type_names` native paths never call Ruby | M3d (code review at the end of the slice) |
| All CI jobs green | M3f |

When closing a slice, also update the corresponding checkbox in the parent
M3 doc. When the last slice closes, the parent acceptance section is fully
checked off and M3 is done.

## Followups

Items deferred from a slice that don't block the slice's own acceptance must
be added to [../../followups.md](../../followups.md) with the trigger that
should pull them in (typically "before M3X starts").
