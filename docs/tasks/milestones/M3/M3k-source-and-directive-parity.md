# M3k: `Environment#sources` / `#declarations` / directive parity

## Goal

Close the remaining behavioral divergence between librbs-backed
`RBS::Environment` and the upstream pure-Ruby implementation: after
`from_loader` and `resolve_type_names`, the four source-derived APIs
(`sources`, `declarations`, `each_rbs_source`, `each_ruby_source`) must
return the same shape, count, and — where upstream guarantees it —
the same Ruby object identity as their pure-Ruby counterparts.

## Background: design evolution and final approach

The first draft of M3k assumed M3h would land as a source-driven
materializer; the realised M3h is **entry-driven** (`build_entries`
iterates `env.*_decls` and looks up each entry's AST via `DeclRef`
independently). Bolting `source.declarations` on as a side-array would
double-materialise nested decls and break the upstream identity
invariant.

The second draft proposed inverting the materialiser to be
source-driven with a `DeclRef → Value` cache, then refactoring the
entry walker into cache lookups. That works, but reproduces in Rust
the indexing logic that upstream's `Environment#add_source` already
implements in Ruby — at the cost of ~300 lines of `process_*` code
plus a cache machinery, plus a multi-stage cutover.

**This slice instead delegates `*_decls` indexing to upstream's
`add_source` entirely.** The Rust side materialises Ruby decl objects
and assembles `RBS::Source::RBS` instances; the Ruby side calls
`add_source(source)` on each, letting upstream populate `@sources`,
`@class_decls`, `@interface_decls`, … as it does for the pure-Ruby
loader path. M3h's `build_entries` / `process_*` becomes dead code
and is retired.

Why this is the right shape:

- **Identity invariant is automatic.** Upstream `add_source` registers
  the same Ruby decl object in `source.declarations` and in the
  matching `*_decls` Entry — at every nesting level. We get this for
  free by using the upstream code path.
- **Less code, less divergence.** `*_decls` ordering, hash semantics,
  duplicate handling, and any future upstream changes flow through to
  librbs without porting.
- **Compat verification simplifies.** M3i / M3j parity reduces to "is
  the `Source::RBS` we pass to `add_source` correct?" — a pure
  AST → Ruby translation question with no `*_decls` shape concerns.
- **Resolved-env path is the same as fresh-env.** A resolved
  `RBS::Environment` materialises by feeding `add_source` the same
  way; upstream's own `resolve_type_names` does exactly that.

The cutover is split into one Rust foundation PR (additive,
Ruby-invisible) followed by three Ruby-side PRs.

## Prerequisites

- M3h merged (`*_decls` materialisation + `materialize_all` cut-over).
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "Single lazy boundary" and "AST → Ruby conversion".
- Read [./M3h-decls-and-cutover.md](./M3h-decls-and-cutover.md) — this
  slice replaces M3h's entry-construction step with an `add_source`
  call.

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
side has no equivalent push path. M3h's `materialize_all` writes the
six `*_decls` ivars directly without going through `add_source`.

After `resolve_type_names`, upstream returns a fresh env whose
`@sources` contains **new** `Source::RBS` / `Source::Ruby` instances
sharing the original `buffer` / `directives` references but carrying
**resolved** decl objects in `declarations`. The librbs native resolve
path swaps the side-table only and does not rebuild sources, so the
divergence persists across resolve.

### Object-identity invariant

Upstream `add_source` registers the same Ruby decl object both in
`source.declarations` and in the corresponding `*_decls` Entry, at
**every nesting level**:

```ruby
# top-level
source.declarations[i].equal?(class_decls[name].decls[k].decl)  # → true
# nested (a class declared inside a module)
source.declarations[i_module].members[j].equal?(class_decls[Foo::Bar].decls[k].decl)  # → true
```

`canonical_dump` does not observe this, but `Marshal.dump`,
`inspect`, and any consumer that cross-references decls via two
paths (Steep does this in some incremental flows) will diverge if
materialisation produces two distinct Ruby objects for the same
logical decl. M3h's current entry walker breaks the nested case
because it materialises nested decls once as parent's `members` and
again as their own `*Entry`. The `add_source`-based cutover in this
slice fixes both cases as a side-effect of using upstream's
indexing.

### Rust-side gap

The Rust `Environment` / `Source` (`crates/librbs-core/src/source.rs`,
`env/mod.rs`) currently model only `*_decls` and the raw parsed AST.
There is no Rust-side analogue to `Source::RBS#directives`: the
resolver walks `parser.signature().directives()` for `Use` clauses
ad-hoc. R2 introduces an owned `Vec<Directive>` so directives can be
consumed by both the resolver and the Y1 directive materialiser
without re-walking the AST.

## Implementation plan

The slice lands as one Rust-side foundation PR (R2) followed by
three Ruby-side PRs (Y1–Y3).

### Phase 1: Rust foundation

#### PR R2: Rust `Directive` types + `Source::directives`

Mirror upstream's directive AST in owned Rust form, and switch the
resolver off raw-AST walking onto this owned representation.

Files:

- `crates/librbs-core/src/directive.rs` (new) or appended to
  `source.rs`:

  ```rust
  pub enum Directive {
      Use(UseDirective),
      ResolveTypeNames(ResolveTypeNamesDirective),
  }

  pub struct UseDirective {
      pub clauses: Vec<UseClause>,
      pub location: AstLocation,
  }

  pub enum UseClause {
      Single {
          type_name: TypeNameSym,
          new_name: Option<Sym>,
          location: AstLocation,
      },
      Wildcard {
          namespace: NamespaceSym,
          location: AstLocation,
      },
  }

  pub struct ResolveTypeNamesDirective {
      pub value: bool,
      pub location: AstLocation,
  }
  ```

  `AstLocation` carries a position pair (`start: u32, end: u32`),
  matching what the AST nodes expose. All names are interned through
  the existing `TypeNameInterner` / `Sym` infrastructure — directives
  carry no borrowed AST data, so `Source::directives` can be moved
  freely.

- `crates/librbs-core/src/source.rs`: add
  `pub directives: Vec<Directive>` to `Source`.

- `crates/librbs-core/src/env/insert.rs::insert_rbs_source`: also
  walk `signature().directives()` and return the populated
  `Vec<Directive>` (`Result<Vec<Directive>>` instead of the current
  `Result<()>`). `Use` clause `type_name`s are already absolute by
  C-parser invariant, so interning is a direct lookup.

- `crates/librbs-core/src/env/mod.rs::from_loader`: receive the
  per-source `Vec<Directive>` from `insert_rbs_source` and write it
  into the matching `Source::directives` before moving sources into
  `env`.

- `crates/librbs-core/src/resolver/driver.rs::apply_use_directive`:
  consume from `&source.directives` instead of walking the AST.
  Behaviour must be byte-for-byte identical (verified by existing
  resolver tests).

Tests:

- Per-fixture: directive count, variant, and contents match
  expectations (a single `Use` with mixed clauses,
  `ResolveTypeNames` with both `true` and `false`).
- Resolver tests stay green (no behaviour change).

Ruby surface: unchanged. The Ruby materialiser does not yet consult
`Source::directives`; that wiring lands in PR Y1.

### Phase 2: Ruby cutover

#### PR Y1: Directives materialiser

Files:

- `ext/librbs/src/materialize/directive.rs` (new): build
  `RBS::AST::Directives::Use` / `Use::SingleClause` /
  `Use::WildcardClause` / `RBS::AST::Directives::ResolveTypeNames`
  from Rust `Directive` values. Locations go through the existing
  `make_location` helper.
- Extract a shared `materialize_namespace` helper from
  `ext/librbs/src/materialize/type_name.rs` for use by
  `WildcardClause`.

Tests (`spec/unit/materialize_directives_spec.rb`):

- Fixture with `# use Foo::Bar` and `# use Foo::*` produces the
  right `Use` directive with `SingleClause` / `WildcardClause`
  instances.
- Fixture with `# resolve-type-names: false` produces a
  `ResolveTypeNames` directive with `value: false`.

No wiring yet — `directive.rs` has no callers from materialisation
proper. Completely additive on the Ruby side.

#### PR Y2: `add_source`-based materialisation (cutover)

Replace M3h's direct `*_decls` ivar writes with per-source
`Source::RBS` construction + an `add_source` call per source.
Upstream's `add_source` then populates `@sources`, `@class_decls`,
…, with full identity invariant.

Files:

- `ext/librbs/src/materialize/source.rs` (new): for each `src in
  env.sources`:
  1. Materialise the buffer (existing
     `MaterializeCtx::buffer()` cache).
  2. Materialise directives from `&src.directives` (R2) via PR Y1's
     `materialize/directive.rs` → `Array[RBS::AST::Directives::*]`.
  3. Iterate `src.parser.signature().declarations()` (filtering
     non-decl nodes via `is_decl_node`) and recursively materialise
     each top-level decl via the existing per-AST-node materialisers
     (`materialize_class_node`, `materialize_module_node`, …),
     producing `Array[RBS::AST::Declarations::*]`. Recursion into
     nested members reuses the same per-AST-node code path that M3h
     already exercises.
  4. Build `RBS::Source::RBS.new(buffer, directives, declarations)`.

- `ext/librbs/src/lib.rs::materialize_all`: replace the existing
  body with:

  1. Set `@__librbs_materialized = true` (or a `@__librbs_materializing`
     guard) **before** the loop, so any accessor re-entered from
     inside upstream's `add_source` short-circuits.
  2. For each Rust source, build the `Source::RBS` value (above) and
     call `env_ruby.funcall("add_source", (source_value,))?`.
  3. Upstream's `add_source` writes `@sources` and the six
     `*_decls` ivars. No Rust-side `*_decls` writes remain.

- `lib/librbs/patches/environment.rb`: extend the accessor patch
  list:

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

- `Source::Ruby` is out of scope (loader emits no Ruby sources).
  Stub the dispatch with `unreachable!("M5: Ruby source
  materialisation")`.

Re-entrancy:

Upstream `add_source` reads / writes `@class_decls` etc. via direct
ivar access in some paths and via `attr_reader` in others. The
`@__librbs_materialized = true` flag must be set **before**
`add_source` is invoked so that a patched accessor reached during
that call short-circuits the materialisation guard. M3h's
re-entrancy guard pattern (`@__librbs_materialized`) carries over
unchanged.

Performance note:

Upstream's `add_source` walks each source's decls in Ruby and
inserts into the `*_decls` Hashes. This replaces the work M3h did in
Rust. For stdlib (~hundreds of sources, thousands of decls) the
extra Ruby work is expected to be in the tens of ms — acceptable for
M3. If benchmarks regress noticeably in M4, port the indexing back
to Rust as a followup; the materialiser interface (Source::RBS
values produced by Rust) remains stable either way.

Tests:

- `env.sources` returns `Array[RBS::Source::RBS]` with the right
  count and order.
- `env.declarations.size` matches a pure-RBS subprocess for the
  curated fixture set.
- Identity invariant at every nesting level:
  `source.declarations[i].equal?(class_decls[name].decls[j].decl)`,
  including nested decls reachable via `members`.
- `each_rbs_source.to_a.size == env.sources.size`,
  `each_ruby_source.to_a` is empty.
- Directive parity (now wired end-to-end through `add_source`).
- Re-entrancy: `materialize_all` twice is a no-op; `@sources`
  retains object identity across repeated accessor calls.
- M3i / M3j compat suite stays green — `*_decls` are now built by
  upstream code so structural parity is by construction.

Rollback story: reverting Y2 returns the codebase to M3h's state
(`*_decls` populated directly, `@sources` empty). No regression of
existing functionality.

#### PR Y3: Retire M3h's entry-construction code

Y2 leaves M3h's `build_entries` / `process_class_like` / `process_*`
unreferenced in production, since `materialize_all` no longer calls
them. Y3 deletes that code and tightens the remaining surface.

Files:

- `ext/librbs/src/materialize/decl.rs`: remove
  `build_entries`, the `EntryHashes` struct, the `process_*`
  functions, the `*Snapshot` structs, and the `materialize_*_decl`
  wrappers that exist only to be called from `process_*`.
- Keep the per-AST-node materialisers (`materialize_class_node`,
  `materialize_module_node`, `materialize_interface_node`,
  `materialize_type_alias_node`, …) — these are now invoked by
  `materialize/source.rs` during recursion.
- Remove `ClassRefs::entry_*` fields no longer referenced (the eight
  `entry_*` Ruby class lookups in `MaterializeCtx::ClassRefs`).
- Restructure `decl.rs` so its public API is "recursive AST → Value
  walker invoked from `source.rs`".

Tests: existing suite stays green.

Ruby surface: unchanged from Y2.

### Resolved-env path

`resolve_type_names` (M3d) returns a fresh `RBS::Environment` with a
new `@__librbs_handle` and `@__librbs_materialized = false`. The
first accessor on the resolved env runs the new
`add_source`-based `materialize_all` against the resolved Rust
handle, producing `Source::RBS` instances whose `declarations` carry
resolved decl trees. Identity invariant holds within the resolved
env because upstream `add_source` is doing the indexing. No extra
code is needed here.

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
array reference; librbs materialises directives per env.

This slice does not fix the cross-env identity. The Buffer itself is
value-equal (same `name`, same `content`), so any consumer that
compares by content rather than `equal?` is unaffected. Track as a
followup gated on a real consumer needing it (Steep doesn't today).

## Out of scope (deferred)

- `Source::Ruby` materialisation → M5 (loader does not produce Ruby
  sources today; the only producer is M5's `add_source` patch).
- Cross-env Buffer / directive identity (`original.sources[i].buffer
  .equal?(resolved.sources[i].buffer)`) → followup, gated on a real
  consumer needing it.
- `Source::RBS#each_type_name` performance — the upstream method
  walks Ruby decls, which now means walking materialised objects.
  Acceptable for M3; if it becomes hot in M4 benchmarks, port to
  Rust.
- Rust-side `*_decls` indexing — Y2 lets upstream's `add_source` do
  this work in Ruby. If M4 benchmarks show regression, porting the
  indexing back into Rust is a followup; Y2's interface (Rust
  produces `Source::RBS`, Ruby calls `add_source`) is stable either
  way.

## Acceptance

Rust foundation:

- [ ] `Source::directives: Vec<Directive>` populated by
      `insert_rbs_source`; resolver consumes from this field.

Ruby cutover:

- [ ] `materialize_all` issues one `env.add_source(Source::RBS.new(…))`
      per Rust source; no direct `*_decls` ivar writes from Rust.
- [ ] `env.sources` returns a populated `Array[RBS::Source::RBS]`
      after the first source-derived (or `*_decls`) accessor call.
- [ ] `env.declarations.size` and shape matches a pure-RBS subprocess
      for the curated fixture set.
- [ ] Object-identity invariant holds within one env at every nesting
      level: `source.declarations[i].equal?(class_decls[name].decls[j].decl)`
      and the analogous assertion for nested decls reachable via
      `members`.
- [ ] `each_rbs_source` / `each_ruby_source` patches trigger
      materialisation and yield the correct sources.
- [ ] Directive materialiser covers `Use` (both clause kinds) and
      `ResolveTypeNames`; round-trip tests pass.
- [ ] Re-entrancy: `materialize_all` twice still a no-op; `@sources`
      retains object identity across repeated accessor calls;
      accessors re-entered from inside `add_source` do not recurse
      into materialisation.
- [ ] Pure-Ruby `RBS::Environment.new` path remains untouched (no
      `@__librbs_handle` → accessors fall through to `super()`).
- [ ] After Y3: M3h's `build_entries` / `process_*` / per-entry
      `materialize_*_decl` wrappers are removed; only the per-AST-node
      materialisers remain in `ext/librbs/src/materialize/decl.rs`.
- [ ] CI green at every PR boundary (R2, Y1, Y2, Y3).

## References

- `vendor/rbs/lib/rbs/source.rb` (Source::RBS / Source::Ruby shape)
- `vendor/rbs/lib/rbs/ast/directives.rb` (Use / ResolveTypeNames)
- `vendor/rbs/lib/rbs/environment.rb:14-16` (`#declarations` definition)
- `vendor/rbs/lib/rbs/environment.rb:455-468` (`add_source` identity
  contract — the upstream code path Y2 delegates to)
- `vendor/rbs/lib/rbs/environment.rb:522-560` (`resolve_type_names`
  source-rebuild — same `add_source` pattern Y2 uses)
- `ext/librbs/src/materialize/mod.rs` (MaterializeCtx, buffer cache)
- `ext/librbs/src/materialize/decl.rs` (current entry-driven walker;
  Y3 retires `build_entries` and `process_*`, keeps per-AST-node
  materialisers)
- `crates/librbs-core/src/source.rs` (Rust Source / Buffer; R2 edit
  target — adds `Source::directives`)
- `crates/librbs-core/src/env/insert.rs::insert_rbs_source` (R2 edit
  target — directive collection)
- `crates/librbs-core/src/env/mod.rs::from_loader` (R2 edit target —
  wires per-source `Vec<Directive>` into `Source`)
- `crates/librbs-core/src/resolver/driver.rs::apply_use_directive`
  (R2 edit target — switch from AST walk to `Source::directives`)
