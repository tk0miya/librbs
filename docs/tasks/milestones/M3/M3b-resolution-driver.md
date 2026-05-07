# M3b: Resolution driver and Rust-side canonical dump

## Goal

Walk every parsed source's AST, record a `ResolvedRef` for each type-name
occurrence into the `Resolution` side-table, and emit a deterministic
canonical dump string entirely from Rust. Still no magnus boundary work.

This is the largest slice of M3 by line count: the AST traversal must cover
every node-type branch in `vendor/rbs/lib/rbs/environment.rb:577-980`. Missing
even one variant will surface later as a canonical-dump diff failure.

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

### `crates/librbs-core/src/canonical.rs`

Define the canonical dump format **as a written specification first**, then
implement against it. Recommended layout:

```
# docs/tasks/milestones/M3/CANONICAL_FORMAT.md   <- new file in this slice
- UTF-8, lines separated by \n
- Top-level entries sorted by TypeName.to_s
- Two-space indentation, no tabs
- Type names always emitted in fully qualified form (::A::B)
- Resolved names use the resolution table; unresolved fall back to original
- Method overload order preserved
- Comments and locations excluded
- ...
```

Then implement `pub fn canonical_dump(env: &Environment, resolution: Option<&Resolution>) -> String`:

- Iterate each of the six entry hashes in TypeName order.
- For each entry, emit declarations in the same order as Ruby's
  `each_decl`.
- Within each declaration, walk the AST in the same order as Ruby's
  canonical dumper (defined alongside in M3c, but spec it here).
- For type-name occurrences, look up the `NodeId` in the resolution table:
  - `Resolved(sym)` → emit fully qualified absolute name
  - `Unresolved(sym)` → emit the original name as-is
  - missing entry → emit original name (env not yet resolved)

The format spec doc must be written before the implementation lands.

### Tests

- Add `crates/librbs-core/tests/resolution.rs`:
  - Build a fixture environment from a small RBS string (or reuse the
    discovery harness on a temp dir).
  - Assert specific `NodeId → ResolvedRef` entries.
  - Assert canonical dump output for a few hand-written cases.
- Round-trip test for the M2 followup
  "DeclRef indexing consistency between insert and lookup": the driver is
  the first reader of `DeclRef`, so add a test that for every entry,
  `decl_index` resolves to a node whose name/kind matches the entry.

## Out of scope (deferred)

- magnus boundary — M3c.
- Ruby-side canonical dump helper — M3c (the format spec written here is
  what M3c implements against on the Ruby side).
- Materialization — M3e.

## Acceptance

- [ ] `crates/librbs-core/src/canonical.rs` exists and produces deterministic
      output for two stable fixture environments (snapshot tested).
- [ ] AST traversal covers every variant of `RBS::AST::Members::*`,
      `RBS::AST::Declarations::*`, `RBS::AST::TypeParam`,
      `RBS::MethodType`, and `RBS::Types::*` referenced in the upstream
      `resolve_*` family. Reviewers spot-check that each Rust branch has a
      Ruby line-range comment.
- [ ] `# resolve-type-names: false` short-circuits resolution for the
      source — covered by a unit test.
- [ ] Format spec at `docs/tasks/milestones/M3/CANONICAL_FORMAT.md` is
      written and matches the implementation.
- [ ] `DeclRef` round-trip test in place.
- [ ] `cargo test -p librbs-core` green.

## References

- `vendor/rbs/lib/rbs/environment.rb:500-560` (driver),
  `:577-980` (per-node walks)
- M2 followup "DeclRef indexing consistency between insert and lookup"
- ruby-rbs v0.3: `Node` variants
