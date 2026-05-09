# M3h: Declarations, entry construction, and `materialize_all` cut-over

## Goal

Land the last layer (`RBS::AST::Declarations::*`), build the six
`Environment::*Entry` Hashes from the materialized decls, wire
`Librbs::Native.materialize_all` into a real implementation, and
patch the six `*_decls` accessors so they trigger materialization on
first access. This slice is where the original M3e acceptance bar
becomes deliverable.

## Prerequisites

- M3e + M3f + M3g merged.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "AST → Ruby conversion", "Patch layer", and the
  `materialize_all` flow under "Native API".

## Scope

### `ext/librbs/src/materialize/decl.rs`

Mirror the exhaustive list in `crates/librbs-core/src/resolver/driver.rs::walk_declaration`.
No wildcard fallthroughs.

| AST node | Ruby class | Notes |
|---|---|---|
| `Class` | `RBS::AST::Declarations::Class` | name (absolutize), type_params, super_class (`Class::Super.new(name:, args:, location:)`), members, annotations, comment |
| `Module` | `RBS::AST::Declarations::Module` | name, type_params, self_types (`Module::Self.new(name:, args:, location:)`), members, annotations, comment |
| `Interface` | `RBS::AST::Declarations::Interface` | name, type_params, members, annotations, comment |
| `TypeAlias` | `RBS::AST::Declarations::TypeAlias` | name, type_params, type, annotations, comment |
| `Constant` | `RBS::AST::Declarations::Constant` | name, type, comment |
| `Global` | `RBS::AST::Declarations::Global` | name (Symbol with `$` prefix), type, comment |
| `ClassAlias` | `RBS::AST::Declarations::ClassAlias` | new_name, old_name (absolutize), comment |
| `ModuleAlias` | `RBS::AST::Declarations::ModuleAlias` | new_name, old_name (absolutize), comment |

`name` for top-level decls is built from the source-side AST name,
absolutized via the parent context — pre-order matching
`crates/librbs-core/src/env/insert.rs::insert_decl`. Nested
declarations recurse with the parent's full name as the prefix.

### `ext/librbs/src/materialize/mod.rs` — entry construction

Build the six `*_decls` Hashes the upstream `RBS::Environment` exposes:

- `class_decls`: `Hash[TypeName, ClassEntry | ModuleEntry]`. Walk
  every source's top-level decls in pre-order; for each `Class` /
  `Module`, look the entry up by absolute name (creating a fresh
  `ClassEntry.new(name)` or `ModuleEntry.new(name)` on first miss),
  then `entry << [context, decl]`. Recurse into class/module
  members for nested decls. Nesting context is the chain of parent
  `TypeName`s (an `Array[TypeName]?` matching upstream's nil-cons
  cell).
- `interface_decls`: `Hash[TypeName, InterfaceEntry]` — one entry
  per name; duplicate detection is upstream's responsibility but
  cannot trigger here because `env::insert::insert_decl` already
  rejected duplicates in M2.
- `type_alias_decls`: `Hash[TypeName, TypeAliasEntry]`.
- `constant_decls`: `Hash[TypeName, ConstantEntry]`.
- `class_alias_decls`: `Hash[TypeName, ClassAliasEntry | ModuleAliasEntry]`.
- `global_decls`: `Hash[Symbol, GlobalEntry]`.

Entry classes (`RBS::Environment::ClassEntry` etc.) are constructed
through magnus by calling `.new` with the right argument shape —
`SingleEntry` subclasses take `(name:, decl:, context:)`,
`ClassEntry` / `ModuleEntry` take a positional `name` and accumulate
via `<<`.

The walk order for entry construction must match `env::insert::insert_decl`'s
pre-order; otherwise the M2 round-trip invariant breaks and downstream
consumers (Steep, `each_decl`) see decls in an unexpected order.

### `ext/librbs/src/lib.rs::materialize_all` (real implementation)

Replace the M3e no-op:

1. If `@__librbs_materialized` is `true`, return `Ok(())`.
2. Extract `@__librbs_handle` (Rust env) and `@__librbs_resolution`
   (optional).
3. For each source: build a `MaterializeCtx`, walk
   `parser.signature().declarations()` in pre-order, and feed the
   results into the entry-construction step above.
4. Set `@class_decls`, `@interface_decls`, `@type_alias_decls`,
   `@constant_decls`, `@class_alias_decls`, `@global_decls` to the
   built Hashes.
5. Set `@__librbs_materialized = true`.

Re-entrancy: a second call observes the flag and returns `Ok(())`
immediately, preserving object identity for the six Hashes.

### `lib/librbs/patches/environment.rb`

Add the accessor patches and the `ensure_materialized` hook:

```ruby
%i[class_decls interface_decls type_alias_decls
   constant_decls class_alias_decls global_decls].each do |m|
  define_method(m) do
    ensure_materialized
    super()
  end
end

private

def ensure_materialized
  return if @__librbs_materialized
  return unless instance_variable_defined?(:@__librbs_handle)
  Librbs::Native.materialize_all(self)
end
```

The `instance_variable_defined?` guard preserves the pure-Ruby path:
`RBS::Environment.new` instances have no Rust handle and fall
through to `super()`.

### Cleanup

- Delete the entire `m3e_test_entries` module in
  `ext/librbs/src/lib.rs` (helpers `first_decl_name` /
  `first_decl_super_name` plus the `materialize_first_*` /
  `materialize_all_decl_locations` functions). The module is wrapped
  with `// ===== M3e temporary test-entry harness =====` markers for
  easy grep.
- Remove the four `_materialize_*` singleton method registrations
  from `init`.
- Remove any other temporary helper Ruby code (e.g. the M3e PR-#10
  `_materialize!` shortcut, if it was reintroduced experimentally).
- Delete the M3e test specs that depend on the removed entries
  (`spec/unit/materialize_location_spec.rb` and
  `spec/unit/materialize_type_name_spec.rb`); the equivalent
  end-to-end coverage moves to M3h's `spec/unit/materialize_spec.rb`
  + the canonical-dump compat tests.
- Demote `librbs_core::env::insert::intern_type_name_node` from `pub`
  back to `pub(crate)`. It was bumped to `pub` in M3e solely so the
  `m3e_test_entries` module (in the `ext/librbs` crate) could intern
  the AST name of a freshly-walked decl. Once that module is gone,
  the only callers are `env::insert` and `resolver::driver` — both
  inside `librbs-core` — so crate-private visibility is sufficient.
  (Do **not** confuse with `is_decl_node`, which was reverted to
  `pub(crate)` in PR #13 already.)

### Tests

`spec/unit/materialize_spec.rb`:

- `env.class_decls` returns a populated Hash whose values are
  `ClassEntry` / `ModuleEntry` with `each_decl` yielding real
  `RBS::AST::Declarations::*` instances; recurse one level into
  members and assert they are `RBS::AST::Members::*`.
- Re-entrancy: calling `materialize_all` twice is a no-op; a second
  accessor call returns the same Hash object.
- Pure-Ruby env (`RBS::Environment.new`) stays untouched —
  accessors return empty Hashes via `super()`, `ensure_materialized`
  early-returns.

`spec/compat/object_spec.rb` (curated stdlib subset):

- For each name in a small set (e.g. `::Object`, `::Integer`,
  `::String`, `::Array`, `::Hash`, `::Numeric`), assert
  `env.class_decls[name].each_decl.first.to_json` matches the
  pure-RBS subprocess's JSON byte-for-byte. Repeat for the
  `resolve_type_names`-applied variant.

`spec/compat/canonical_dump_core_spec.rb`:

- Drop `pending`. The librbs-side dump must match the pure-RBS dump
  for both unresolved and resolved core.

## Out of scope (deferred)

- core+stdlib / gems compat matrix → M3i.
- Per-Entry lazy materialization → M4 (decision point).
- Performance benchmarks → M4.
- `RBS::AST::Ruby::*` (inline ruby annotations) — these only appear
  via `Source::Ruby`, which the loader never produces today; if M5
  adds Ruby-source ingestion, the materializer extends here.

## Acceptance

- [ ] `Librbs::Native.materialize_all(env)` populates the six
      `*_decls` ivars with real `RBS::AST::*` instances.
- [ ] Re-entrancy: calling `materialize_all` twice is a no-op.
- [ ] Patched accessors auto-trigger materialization and return the
      same object on repeat calls.
- [ ] Per-entry equality spec passes against the curated stdlib
      subset for both unresolved and resolved envs (full matrix is
      M3i).
- [ ] Canonical-dump compat (core / resolved core) is no longer
      `pending` and passes.
- [ ] Multi-byte regression (from M3e) remains green through the
      cut-over.
- [ ] All M3a–M3g test-only entry points are removed.
- [ ] CI green.

## References

- `vendor/rbs/lib/rbs/ast/declarations.rb`
- `vendor/rbs/lib/rbs/environment/class_entry.rb`
- `vendor/rbs/lib/rbs/environment/module_entry.rb`
- `vendor/rbs/lib/rbs/environment.rb` (SingleEntry subclasses)
- `crates/librbs-core/src/env/insert.rs` (pre-order walk to mirror)
- `crates/librbs-core/src/resolver/driver.rs::walk_declaration`
