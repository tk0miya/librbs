# M3b: Resolution driver

## Goal

Walk every parsed source's AST and record a `ResolvedRef` for each
type-name occurrence into the `Resolution` side-table. Still no magnus
boundary work.

The AST traversal must cover every node-type branch in
`vendor/rbs/lib/rbs/environment.rb:577-980`. Missing even one variant
will surface later as a canonical-dump diff failure once M3c starts
running compatibility checks.

The canonical-dump format spec and Rust-side dumper were originally
part of this slice; both moved to M3c so the spec lives next to its
implementation. See the followup "Rust-side `canonical_dump`
implementation" for the eventual Rust port.

## Prerequisites

- M3a merged.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "Resolution driver", "Parallelization", "Canonical dump",
  and "Pitfalls and mitigation > Missed AST traversal cases" /
  "Aligning canonical-dump format" / "Honoring `resolve-type-names: false`".
- Study every line of `vendor/rbs/lib/rbs/environment.rb` between 577 and 980
  (`resolve_type_names`, `resolve_declaration`, `resolve_member`,
  `resolve_method_type`, `resolve_type_params`, ...). Transcribe each Ruby
  line as a comment above the Rust counterpart so reviewers can verify
  line-by-line correspondence.
- Review the `ruby-rbs` v0.3 crate's `Node` enum surface for the variants
  the driver must visit.

## Scope

### `crates/librbs-core/src/resolver/driver.rs`

```rust
pub fn resolve(env: &Environment) -> Resolution {
    let mut resolver = TypeNameResolver::build(env);
    let mut table = UseMap::Table::new();
    table.populate_from(env);
    table.compute_children();

    env.sources.par_iter().enumerate()
        .map(|(idx, source)| resolve_source(idx as u32, source, env, &resolver, &table))
        .reduce(Resolution::default, Resolution::merge)
}
```

`resolve_source` performs:

1. Construct a `UseMap` from the source's `# use ...` directives. The
   directive walking code (clause shape, alias-vs-rename cases) lives here,
   not in M3a's `UseMap`.
2. Honor `# resolve-type-names: false` magic comment by short-circuiting
   the source (no entries written into the resolution).
3. Walk every top-level declaration with a `walk_declaration(decl, ctx, ...)`
   recursion. For every type-name occurrence, call `resolver.resolve(...)`
   and insert into `Resolution`.

### AST node coverage

Mirror these Ruby methods one-for-one. Each gets a Rust `walk_*` function:

- `resolve_declaration` → class / module / class-alias / module-alias /
  interface / type-alias / constant / global.
- `resolve_member` → `MethodDefinition`, `Include`, `Extend`, `Prepend`,
  `Attr{Reader,Writer,Accessor}`, `Var{Instance,ClassInstance,Class}`,
  `Public`, `Private`, `Alias`.
- `resolve_method_type` → method-type, type params, function (positional /
  keyword / rest), block, return type.
- `resolve_type` → every variant of `RBS::Types::*`: `ClassInstance`,
  `Interface`, `Alias`, `ClassSingleton`, `Tuple`, `Record`, `Union`,
  `Intersection`, `Optional`, `Proc`, `Block`, `RecordField`, `Variable`,
  `Bases::*`, `Literal`.
- `resolve_type_params` → upper bound, default, etc.

For each branch, write a comment quoting the corresponding Ruby line range,
e.g. `// environment.rb:612-618`.

### Canonical dump (deferred to M3c)

The canonical-dump format spec and the dumper that walks it have been
moved to M3c so the spec lives next to its implementation. M3b only
ensures the resolution side-table contains everything the dumper will
need (per-occurrence `Resolved` / `Unresolved` records, plus
round-trippable `DeclRef`s). See the followup "Rust-side
`canonical_dump` implementation" if/when a Rust port becomes
necessary as a CI optimization.

### Tests

- Add `crates/librbs-core/tests/resolution.rs`:
  - Build a fixture environment from a small RBS string (or reuse the
    discovery harness on a temp dir).
  - Assert specific `NodeId → ResolvedRef` entries.
- Round-trip test for the M2 followup
  "DeclRef indexing consistency between insert and lookup": the driver is
  the first reader of `DeclRef`, so add a test that for every entry,
  `decl_index` resolves to a node whose name/kind matches the entry.

## Out of scope (deferred)

- magnus boundary — M3c.
- Canonical-dump format spec and Ruby-side dumper — M3c (spec and
  implementation kept together to avoid drift).
- Rust-side canonical dumper — followup "Rust-side `canonical_dump`
  implementation"; revisited only if the Ruby dumper becomes a CI
  bottleneck.
- Materialization — M3e/M3f/M3g/M3h.

## Acceptance

- [x] AST traversal covers every variant of `RBS::AST::Members::*`,
      `RBS::AST::Declarations::*`, `RBS::AST::TypeParam`,
      `RBS::MethodType`, and `RBS::Types::*` referenced in the upstream
      `resolve_*` family. Reviewers spot-check that each Rust branch has a
      Ruby line-range comment.
- [x] `# resolve-type-names: false` short-circuits resolution for the
      source — covered by a unit test.
- [x] `DeclRef` round-trip test in place.
- [x] `cargo test -p librbs-core` green.

The canonical-dump format spec was originally part of M3b's scope but
has been moved to M3c, where the dumper itself lives — keeping spec
and implementation in the same slice avoids drift. See the followup
"Rust-side `canonical_dump` implementation" for the deferred Rust
port.

## References

- `vendor/rbs/lib/rbs/environment.rb:500-560` (driver),
  `:577-980` (per-node walks)
- M2 followup "DeclRef indexing consistency between insert and lookup"
- ruby-rbs v0.3: `Node` variants
