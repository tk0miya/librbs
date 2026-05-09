# M3 Subtask Index

M3 (the "main milestone") is too large to deliver in a single session. It is
split into the nine self-contained slices below. Each slice is intended to
land as one PR and to be picked up by a fresh agent session.

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
| [M3e](M3e-materialization.md) | Materialization plumbing: `MaterializeCtx`, `RBS::Location`, `RBS::TypeName`, NodeId walk skeleton | M3d |
| [M3f](M3f-types-and-type-params.md) | `RBS::Types::*` + `RBS::AST::TypeParam` | M3e |
| [M3g](M3g-method-types-and-members.md) | `RBS::MethodType` + `RBS::AST::Members::*` | M3f |
| [M3h](M3h-decls-and-cutover.md) | `RBS::AST::Declarations::*` + `Environment::*Entry` + `materialize_all` cut-over + accessor patches | M3g |
| [M3i](M3i-patches-and-compat.md) | Patches polish + core+stdlib + major-gems compat matrix | M3h |

The original M3 plan had a single materialization slice (old M3e) covering
every AST variant. The PR-#10 retrospective (an attempted single-slice
implementation degenerated into a Ruby-side re-parse shortcut that bypassed
M3a–M3d entirely) split it into M3e/M3f/M3g/M3h so each AST layer can be
designed, reviewed, and unit-tested independently. The cut-over to the
patched accessors lands only at M3h.

## Acceptance mapping

The six acceptance checkboxes in the parent M3 doc are satisfied as follows:

| Acceptance item (parent) | Slice that closes it |
|---|---|
| All `cargo test -p librbs-core` tests are green | M3a, M3b |
| Canonical dumps for core only match pure RBS exactly | M3h (Ruby-side helper exists from M3c, but needs materialization to walk a populated env) and verified again at M3i |
| Canonical dumps for core + stdlib match pure RBS exactly | M3i |
| Major-gems matrix is green | M3i |
| `from_loader` / `resolve_type_names` native paths never call Ruby | M3d (initial), re-audited at M3i now that materialization adds Ruby-call surface |
| All CI jobs green | M3i |

When closing a slice, also update the corresponding checkbox in the parent
M3 doc. When the last slice closes, the parent acceptance section is fully
checked off and M3 is done.

## Followups

Items deferred from a slice that don't block the slice's own acceptance must
be added to [../../followups.md](../../followups.md) with the trigger that
should pull them in (typically "before M3X starts").
