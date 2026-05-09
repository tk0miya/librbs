# M3k: `Environment#sources` / `#declarations` / directive parity

## Goal

Close the remaining behavioral divergence between librbs-backed
`RBS::Environment` and the upstream pure-Ruby implementation: after
`from_loader` and `resolve_type_names`, the four source-derived APIs
(`sources`, `declarations`, `each_rbs_source`, `each_ruby_source`) must
return the same shape, count, and — where upstream guarantees it —
the same Ruby object identity as their pure-Ruby counterparts.

This slice is purely about compatibility surface; no new resolver or
materializer logic is added beyond directive nodes.

## Prerequisites

- M3h merged (`*_decls` materialization + `materialize_all` cut-over).
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "Single lazy boundary" and "AST → Ruby conversion".
- Read [./M3h-decls-and-cutover.md](./M3h-decls-and-cutover.md) — this
  slice extends M3h's materialization session.

## Problem statement

Upstream `RBS::Environment` exposes four source-derived APIs that
librbs leaves empty today:

| API | Upstream | librbs after M3h |
|---|---|---|
| `sources` (`attr_reader`) | populated by `add_source` | `[]` |
| `declarations` (`sources.flat_map(&:declarations)`) | every top-level decl | `[]` |
| `each_rbs_source` / `each_ruby_source` | filters `sources` by class | yields nothing |

`build_environment` (`ext/librbs/src/lib.rs`) calls
`RBS::Environment#initialize` to set up the standard ivars, then
attaches `@__librbs_handle`. `@sources` stays `[]` because the native
side has no equivalent push path. `materialize_all` only writes the
six `*_decls` ivars.

After `resolve_type_names`, upstream returns a fresh env whose
`@sources` contains **new** `Source::RBS` / `Source::Ruby` instances
sharing the original `buffer` / `directives` references but carrying
**resolved** decl objects in `declarations`. The librbs native resolve
path swaps the side-table only and does not rebuild sources, so the
divergence persists across resolve.

### Object-identity invariant

Upstream `add_source` registers the same Ruby decl object both in
`source.declarations` and in the corresponding `*_decls` Entry:

```ruby
source.declarations[i].equal?(class_decls[name].decls[k].decl)  # → true
```

`canonical_dump` does not observe this, but `Marshal.dump`,
`inspect`, and any consumer that cross-references decls via two
paths (Steep does this in some incremental flows) will diverge if
materialization produces two distinct Ruby objects for the same
logical decl.

## Scope

### `ext/librbs/src/materialize/directive.rs` (new)

Mirror the two AST node families in `vendor/rbs/lib/rbs/ast/directives.rb`:

| Rust node (via `signature().directives()`) | Ruby class | Notes |
|---|---|---|
| `UseNode` | `RBS::AST::Directives::Use` | `clauses:` is an array of `SingleClause` / `WildcardClause`; `location:` covers the directive line |
| `UseSingleClauseNode` | `RBS::AST::Directives::Use::SingleClause` | `type_name:` (absolutize via the same path as decls), `new_name:` (`Symbol` or `nil`), `location:` |
| `UseWildcardClauseNode` | `RBS::AST::Directives::Use::WildcardClause` | `namespace:` (`RBS::Namespace`), `location:` |
| `ResolveTypeNamesNode` | `RBS::AST::Directives::ResolveTypeNames` | `value:` (`true` / `false`), `location:` |

Directives are **not affected by `resolve_type_names`** — the
`Use` clause's `type_name` is already absolute by C-parser invariant,
and `ResolveTypeNames` carries a boolean. Materialize from the AST
directly without consulting `Resolution`.

`RBS::Namespace` for `WildcardClause` is built from the upstream
`Namespace.parse` shape (`"::Foo::Bar::"`); the existing `type_name`
materializer already constructs `Namespace` values internally and
that helper should be extracted / reused.

### `ext/librbs/src/materialize/source.rs` (new)

For each `librbs_core::source::Source`, build one of:

```ruby
RBS::Source::RBS.new(buffer, directives, declarations)
RBS::Source::Ruby.new(buffer, prism_result, declarations, diagnostics)
```

- `buffer`: reuse `MaterializeCtx::buffer()` (already cached per
  source index — same identity as the buffer threaded through
  `RBS::Location` materialization).
- `directives`: built from the per-source directive walk above.
- `declarations`: **the same Ruby `Value`s** that M3h's entry
  construction step pushed into the `*_decls` Entries, in the
  source's top-level pre-order. See "Identity invariant" below.

`Source::Ruby` is **out of scope for this slice**: today's loader
emits no Ruby sources (the only producer is M5's `add_source` path).
Add the `materialize_ruby_source` stub and the dispatch (`match
src.tag`), but mark the Ruby branch `unreachable!("M5: Ruby source
materialization")` and document the gap in the M5 doc.

### `ext/librbs/src/materialize/mod.rs` — entry construction extension

Today (per M3h's design) the entry-construction walk in
`materialize_all` does, for each source:

1. Walk top-level decls in pre-order.
2. Materialize each decl into a Ruby `Value`.
3. Look up / create the `*Entry` and `entry << [context, decl_value]`.

Extend step 2/3 so the same `decl_value` is **also** appended to a
per-source `RArray` that becomes `Source::RBS#declarations`.
Concretely:

```rust
for (src_idx, src) in env.sources.iter().enumerate() {
    ctx.switch_source(src_idx as u32);
    let directives_ary = build_directives(&mut ctx, src)?;
    let decls_ary = ruby.ary_new();
    for top_decl_node in src.parser.signature().declarations().iter() {
        let decl_value = materialize_declaration(&mut ctx, top_decl_node, /* ctx-stack */ &[])?;
        decls_ary.push(decl_value)?;            // for Source#declarations
        push_into_entry(env_ruby, &decl_value)?; // for *_decls (existing M3h logic)
    }
    let source_value = build_source(&mut ctx, src, directives_ary, decls_ary)?;
    sources_ary.push(source_value)?;
}
ivar_set(env_ruby, "@sources", sources_ary)?;
```

Nested decls (members of a Class / Module) are **not** added to
`source.declarations` — upstream only stores top-level decls there;
nested decls are reachable via `Class#each_decl`. The recursion that
M3h does for nested entry construction is unchanged.

### `ext/librbs/src/lib.rs::materialize_all`

Add `@sources` to the list of ivars set inside the
re-entrancy-guarded body. The flag (`@__librbs_materialized`) and the
two-phase ordering (build everything first, then publish) are
unchanged from M3h.

### `lib/librbs/patches/environment.rb`

Extend the accessor patch list so any source-derived API also
triggers materialization:

```ruby
%i[class_decls interface_decls type_alias_decls
   constant_decls class_alias_decls global_decls
   sources declarations].each do |m|
  define_method(m) do
    ensure_materialized
    super()
  end
end

%i[each_rbs_source each_ruby_source].each do |m|
  define_method(m) do |&block|
    ensure_materialized
    super(&block)
  end
end
```

The `instance_variable_defined?(:@__librbs_handle)` guard from M3h
already protects the pure-Ruby path; nothing else changes.

### Resolved-env path

`resolve_type_names` (M3d) returns a fresh `RBS::Environment` with a
new `@__librbs_handle` and `@__librbs_materialized = false`. The first
accessor on the resolved env runs `materialize_all` against the
resolved Rust handle, producing **new** `Source::RBS` instances whose
`declarations` carry resolved decl trees. This matches upstream's
"new sources, new decls" behavior for the resolve path automatically;
no extra code is needed here.

### Buffer identity across envs (documented gap)

Upstream `resolve_type_names` reuses the original `source.buffer`
reference verbatim:

```ruby
env.add_source(Source::RBS.new(source.buffer, source.directives, decls))
```

so `original_env.sources[i].buffer.equal?(resolved_env.sources[i].buffer)`
is `true`. librbs builds a fresh `RBS::Buffer` per env's
`MaterializeCtx`, so this identity does **not** hold across envs.

The same issue applies to `directives` — upstream reuses the same
array reference; librbs materializes directives per env.

This slice does not fix the cross-env identity. The Buffer itself is
value-equal (same `name`, same `content`), so any consumer that
compares by content rather than `equal?` is unaffected. Track as a
followup gated on a real consumer needing it (Steep doesn't today).

### Tests

`spec/unit/materialize_sources_spec.rb`:

- After accessing any `*_decls`, `env.sources` is non-empty and each
  element is an `RBS::Source::RBS` with a populated `directives` and
  `declarations`.
- `env.declarations.size` equals the sum of top-level decl counts
  reported by walking `parser.signature().declarations()` for each
  source (compute the expected count via a fresh pure-RBS subprocess
  parse).
- Identity invariant: for a fixture with a single class decl,
  `env.sources.first.declarations.first.equal?(env.class_decls[name].decls.first.decl)`
  is `true`.
- `env.each_rbs_source.to_a.size == env.sources.size` and
  `env.each_ruby_source.to_a` is empty (loader-only path).
- Resolve identity: after `env.resolve_type_names`, the returned env
  has its own `sources` array distinct from the original, but each
  resolved source's `declarations` shares object identity with the
  resolved env's `*_decls` entries (intra-env invariant preserved).

`spec/unit/materialize_directives_spec.rb`:

- Fixture with `# use Foo::Bar` and `# use Foo::*` produces the right
  `Use` directive with `SingleClause` / `WildcardClause` instances.
- Fixture with `# resolve-type-names: false` produces a
  `ResolveTypeNames` directive with `value: false`.

`spec/compat/source_parity_spec.rb` (curated subset — share fixtures
with M3h's `object_spec.rb`):

- For each fixture, run the librbs-patched env and a pure-RBS
  subprocess; assert `env.declarations.map(&:to_json)` matches
  byte-for-byte and `env.sources.size` matches.

## Out of scope (deferred)

- `Source::Ruby` materialization → M5 (loader does not produce Ruby
  sources today; the only producer is M5's `add_source` patch).
- Cross-env Buffer / directive identity (`original.sources[i].buffer
  .equal?(resolved.sources[i].buffer)`) → followup, gated on a real
  consumer needing it.
- `Source::RBS#each_type_name` performance — the upstream method walks
  Ruby decls, which now means walking materialized objects. Acceptable
  for M3; if it becomes hot in M4 benchmarks, port to Rust.

## Acceptance

- [ ] `env.sources` returns a populated `Array[RBS::Source::RBS]`
      after the first `*_decls` (or any sources-derived) access.
- [ ] `env.declarations.size` and shape matches a pure-RBS subprocess
      for the curated fixture set.
- [ ] Object-identity invariant holds within one env:
      `source.declarations[i].equal?(class_decls[name].decls[j].decl)`.
- [ ] `each_rbs_source` / `each_ruby_source` patches trigger
      materialization and yield the correct sources.
- [ ] Directive materializer covers `Use` (both clause kinds) and
      `ResolveTypeNames`; round-trip tests pass.
- [ ] Re-entrancy: `materialize_all` twice still a no-op; sources
      array identity preserved across repeated accessor calls.
- [ ] Pure-Ruby `RBS::Environment.new` path remains untouched (no
      `@__librbs_handle` → accessors fall through to `super()`).
- [ ] CI green.

## References

- `vendor/rbs/lib/rbs/source.rb` (Source::RBS / Source::Ruby shape)
- `vendor/rbs/lib/rbs/ast/directives.rb` (Use / ResolveTypeNames)
- `vendor/rbs/lib/rbs/environment.rb:14-16` (`#declarations` definition)
- `vendor/rbs/lib/rbs/environment.rb:455-468` (`add_source` identity contract)
- `vendor/rbs/lib/rbs/environment.rb:522-560` (`resolve_type_names` source-rebuild)
- `ext/librbs/src/materialize/mod.rs` (MaterializeCtx, buffer cache)
- `crates/librbs-core/src/source.rs` (Rust Source / Buffer)
- `crates/librbs-core/src/resolver/driver.rs::apply_use_directive`
  (the directive walk we mirror on the materialization side)
