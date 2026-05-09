# M3: Environment and Type-Name Resolution (**main milestone**)

> **This milestone is delivered as nine PRs, not one.**
> See [M3/README.md](M3/README.md) for the slice index. The rest of this
> document is the shared design reference; each slice's own doc states its
> scope, dependencies, and per-slice acceptance.

## Goal

Implement a side-table-based `TypeNameResolver` so that
`RBS::Environment.from_loader` and `#resolve_type_names` become **pure-Rust
operations**. Establish the single lazy boundary by materializing all six
`*_decls` Hashes on the first access.

Compatibility against pure RBS is verified mechanically through
**canonical-dump diff**.

## Prerequisites

- M2 is complete (discovery + parse + Rust-side entry construction).
- The `Environment` struct already has a `resolution: Option<Resolution>`
  field.
- See [../reference.md](../reference.md), section "Referenced in M3" — read
  it carefully. In particular, study `vendor/rbs/lib/rbs/environment.rb:500-560`
  (`resolve_type_names`) and
  `vendor/rbs/lib/rbs/resolver/type_name_resolver.rb`.

## Deliverables

### Rust side

```
crates/librbs-core/src/
├── resolver/
│   ├── mod.rs
│   ├── type_name.rs        # Rust port of TypeNameResolver
│   └── constant.rs         # ConstantResolver (if needed)
├── env/
│   └── resolution.rs       # ResolvedEnvironment / Resolution side-table
├── canonical.rs            # canonical dump for compatibility tests
└── ...
```

### Ruby side

```
lib/librbs/patches/
├── environment_loader.rb
└── environment.rb
```

### Native API

```rust
// added to crates/librbs-ruby/src/lib.rs
fn build_environment(loader: Value) -> Result<Value, Error>;
fn resolve_type_names(env: Value, only: Option<Value>) -> Result<Value, Error>;
fn materialize_all(env: Value) -> Result<(), Error>;
fn canonical_dump(env: Value) -> Result<String, Error>;   // for testing
```

The return values of `build_environment` and `resolve_type_names` are
**genuine `RBS::Environment` instances** carrying a Rust handle in an ivar.

## Tasks

The numbered tasks below are the original task list. Each is now owned by
one of the slices listed in [M3/README.md](M3/README.md):

| Task | Slice |
|---|---|
| 1. Port TypeNameResolver | M3a |
| 2. Port UseMap | M3a (data structures) + M3b (clause walking) |
| 3. Resolution side-table | M3a |
| 4. Resolution driver | M3b |
| 5. Parallelization | M3b |
| 6. Canonical dump | M3b (Rust side, format spec) + M3c (Ruby side helper) |
| 7. Native API | M3c (`build_environment`, `canonical_dump`) + M3d (`resolve_type_names`) + M3e (`materialize_all` no-op stub) + M3h (`materialize_all` cut-over) |
| 8. AST → Ruby conversion | M3e (plumbing: `MaterializeCtx`, `RBS::Location`, `RBS::TypeName`) + M3f (`RBS::Types::*`, `RBS::AST::TypeParam`) + M3g (`RBS::MethodType`, `RBS::AST::Members::*`) + M3h (`RBS::AST::Declarations::*`, `Environment::*Entry`) |
| 9. Patch layer | M3c (loader + `from_loader`) + M3d (`resolve_type_names`) + M3h (accessor patches + `ensure_materialized`) + M3i (hardening) |
| 10. Compatibility tests | M3c (core unresolved) + M3d (core resolved) + M3h (per-entry curated subset, canonical-dump unblocked) + M3i (core+stdlib + gems) |
| 11. Test matrix | M3i |

The remaining sections describe the design contract that all slices share.

### 1. Port TypeNameResolver

Port `vendor/rbs/lib/rbs/resolver/type_name_resolver.rb` to Rust:

- `TypeNameResolver { all_names: FxHashSet<TypeNameSym>, aliases: FxHashMap<TypeNameSym, (TypeNameSym, ContextSym)>, cache: FxHashMap<(TypeNameSym, ContextSym), Option<TypeNameSym>> }`
- `build(env: &Environment) -> Self`
- `resolve(type_name, context) -> Option<TypeNameSym>`
- Port `resolve_namespace0`, `resolve_head_namespace`, `normalize_namespace`,
  etc. faithfully.

Use **the same algorithm**. Do not "optimize" the logic; we will already
gain enough from interning and parallelism. Optimizations risk diff failures
later.

### 2. Port UseMap

Port `vendor/rbs/lib/rbs/environment/use_map.rb`:

- `UseMap::Table`: `known_types: FxHashSet<TypeNameSym>`,
  `children: FxHashMap<NamespaceSym, FxHashSet<TypeNameSym>>`.
- `UseMap`: `use_dirs`, `map: FxHashMap<SymbolSym, TypeNameSym>`.
- `build_map(clause)`, `resolve(type_name) -> TypeNameSym`.

### 3. Resolution side-table

```rust
pub struct Resolution {
    /// Resolved type name for each declaration node position.
    /// Keys are stable AST node IDs (e.g. (source_index, node_offset)).
    pub type_name_resolutions: FxHashMap<NodeId, ResolvedRef>,
}

pub enum ResolvedRef {
    Resolved(TypeNameSym),
    /// When resolution fails, hold the original name (matches Ruby's `|| type_name`).
    Unresolved(TypeNameSym),
}
```

`NodeId` design matters. We need an ID for every type-name occurrence that
is **deterministic and computable in parallel**. Options:

- A pair `(source_index: u32, ast_node_offset: u32)`, where
  `ast_node_offset` is the node offset within ruby-rbs's arena.
- A serial number assigned during a sequential walk of one source.

Pick whichever is easier to implement; deterministic is sufficient.

### 4. Resolution driver

Implement a Rust equivalent of `resolve_type_names` from
`vendor/rbs/lib/rbs/environment.rb:522-560` **without any AST reconstruction**:

```rust
pub fn resolve(env: &Environment) -> Result<Resolution, Error> {
    let resolver = TypeNameResolver::build(env);
    let mut table = UseMap::Table::new();
    table.populate_from(env);
    table.compute_children();

    let mut resolution = Resolution::default();

    // Per-source loop. Optionally parallelize with rayon and merge.
    for source in &env.sources {
        let mut use_map = UseMap::new(&table);
        for dir in &source.directives {
            if let Directive::Use(u) = dir {
                for clause in &u.clauses {
                    use_map.build_map(clause);
                }
            }
        }

        // Honor `# resolve-type-names: false`
        let should_resolve = !source.directives.iter()
            .any(|d| matches!(d, Directive::ResolveTypeNames(r) if !r.value));

        if should_resolve {
            for decl in &source.declarations {
                walk_declaration(decl, &resolver, &use_map, &mut resolution, ...);
            }
        }
    }

    Ok(resolution)
}
```

`walk_declaration` only **descends** the AST (it never reconstructs nodes).
On reaching a node, for each occurring type name, call
`resolver.resolve(type_name, context)` and record the result in
`resolution.type_name_resolutions`.

You will also need equivalents of `resolve_member`, `resolve_method_type`,
`resolve_type_params`, etc. **Read every line of
`vendor/rbs/lib/rbs/environment.rb:577-980` to enumerate the AST node-type
branches and port them all** — missing any single variant will break the
compatibility diff later.

### 5. Parallelization

Convert `for source in env.sources` into `env.sources.par_iter()`. Each
source produces a disjoint `Resolution`; merge them at the end.

### 6. Canonical dump

A deterministic string for compatibility testing. Must be **computed
entirely in Rust** (no Ruby detour, otherwise materialization triggers and
defeats M3).

```rust
pub fn canonical_dump(env: &Environment) -> String {
    // Iterate entries sorted by TypeName,
    // emit each entry's decls in the same order as Ruby,
    // resolve each type name through the resolution table,
    // use fixed indentation/newlines.
}
```

Format is up to you, but the **same dumper must also be implemented for pure
RBS** (described below). JSON or a custom s-expression-like format works
well. Use ordered output everywhere.

Pure-RBS-side dumper (called from spec):

```ruby
def canonical_dump(env)
  out = +""
  env.class_decls.keys.sort_by(&:to_s).each do |name|
    entry = env.class_decls[name]
    out << "class #{name}\n"
    entry.each_decl do |decl|
      out << "  decl: #{decl.class}, super: #{decl.respond_to?(:super_class) ? decl.super_class&.name : nil}\n"
      decl.members.each do |m|
        out << "    member: #{m.class} #{m.respond_to?(:name) ? m.name : ''}\n"
        # ... recurse into method types and type expressions
      end
    end
  end
  # interface_decls, type_alias_decls, ... follow the same pattern
  out
end
```

The **Rust output and Ruby output must match exactly**. Choose a
line-oriented, sorted format so that diffs are easy to debug.

### 7. Native API

```rust
#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("Librbs")?.define_module("Native")?;

    module.define_singleton_method("build_environment", function!(build_environment, 1))?;
    module.define_singleton_method("resolve_type_names", function!(resolve_type_names, 2))?;
    module.define_singleton_method("materialize_all", function!(materialize_all, 1))?;
    module.define_singleton_method("canonical_dump", function!(canonical_dump_native, 1))?;

    Ok(())
}
```

`build_environment` flow:

1. Read `core_root`, `repository`, `libs`, `dirs` from the supplied
   `RBS::EnvironmentLoader` (via Ruby ivar access or compatibility methods).
2. Repackage into a Rust-side `Loader`.
3. Call `Environment::from_loader(&loader)`.
4. Use `RBS::Environment.allocate` to obtain an empty Ruby instance.
5. Set ivar `@__librbs_handle` to a `TypedData`-wrapped `Arc<Environment>`.
6. Return that `RBS::Environment`.

`resolve_type_names` flow:

1. Extract `@__librbs_handle` from the input env.
2. Compute `resolve(env) -> Resolution` in Rust.
3. Construct a new `Environment` that **shares** arena/sources/entries via
   `Arc` and sets `resolution: Some(Arc::new(resolution))`.
4. Return a fresh `RBS::Environment.allocate`.

`materialize_all` flow:

1. Extract `Arc<Environment>` from `@__librbs_handle`.
2. If already materialized (check ivar `@__librbs_materialized`), return.
3. Convert all `class_decls` / `interface_decls` / `type_alias_decls` /
   `constant_decls` / `class_alias_decls` / `global_decls` into real Ruby
   Hashes plus genuine RBS classes.
4. Set the env's `@class_decls` etc. ivars directly.
5. Set `@__librbs_materialized = true`.

### 8. AST → Ruby conversion

This is the bulk of M3 implementation work. Two options for converting from
the C/Rust AST to the Ruby AST:

(a) **Reuse `ast_translation.c`**: Bridge through `rb_funcall` to construct
    `RBS::AST::Declarations::Class.new(...)` etc. Use information from the
    `from_raw` family in `rust/ruby-rbs/src/node/mod.rs`, substituting type
    names via the resolution side-table.

(b) **Reimplement in Rust**: Use magnus to call
    `RBS::AST::Declarations::Class.new(name:, ...)` and friends directly,
    with `RBS::TypeName.new(...)` for type names.

We recommend **(b)** to avoid binding to the C extension. The cost is many
Ruby method calls (hundreds of thousands for stdlib), so materialization
itself is non-trivial.

#### Type-name lookup during materialization

When you reach a node, look up its `NodeId` in
`resolution.type_name_resolutions`:

- `Some(Resolved(sym))` → construct `RBS::TypeName.new(...)` from the
  resolved symbol.
- `Some(Unresolved(sym))` → construct from the original symbol (do not call
  `absolute!`).
- `None` → the env is unresolved (`from_loader` result). Use the AST's
  original name.

### 9. Patch layer

```ruby
# lib/librbs/patches/environment.rb
module Librbs
  module Patches
    module Environment
      module ClassMethods
        def from_loader(loader)
          Librbs::Native.build_environment(loader)
        end
      end

      def resolve_type_names(only: nil)
        Librbs::Native.resolve_type_names(self, only)
      end

      def class_decls
        ensure_materialized
        super
      end

      def interface_decls
        ensure_materialized
        super
      end

      def type_alias_decls
        ensure_materialized
        super
      end

      def constant_decls
        ensure_materialized
        super
      end

      def class_alias_decls
        ensure_materialized
        super
      end

      def global_decls
        ensure_materialized
        super
      end

      private

      def ensure_materialized
        return if @__librbs_materialized
        Librbs::Native.materialize_all(self)
      end
    end
  end
end

RBS::Environment.singleton_class.prepend(Librbs::Patches::Environment::ClassMethods)
RBS::Environment.prepend(Librbs::Patches::Environment)
```

If `ensure_materialized` is invoked on an env without `@__librbs_handle`
(constructed via the pure Ruby path), `super` must continue to work. Add a
nil check inside the patch for fallback.

### 10. Compatibility tests

```ruby
# spec/compat/resolve_type_names_spec.rb
require "rbs"

RSpec.describe "resolve_type_names compatibility" do
  let(:loader) { RBS::EnvironmentLoader.new }

  before do
    # core only at first; expand to stdlib + major gems later
  end

  it "matches RBS canonical dump for core" do
    # Compute via pure RBS
    pure_dump = without_librbs do
      env = RBS::Environment.from_loader(loader).resolve_type_names
      canonical_dump(env)
    end

    # Compute via librbs
    require "librbs"
    librbs_dump = begin
      env = RBS::Environment.from_loader(loader).resolve_type_names
      canonical_dump(env)
    end

    expect(librbs_dump).to eq(pure_dump)
  end
end
```

`without_librbs` temporarily disables the librbs patches. Where temporary
disabling isn't reliable, run a fresh Ruby subprocess:

```ruby
# spec/support/subprocess.rb
def dump_with_pure_rbs(loader_setup)
  script = <<~RUBY
    require "rbs"
    loader = RBS::EnvironmentLoader.new
    #{loader_setup}
    env = RBS::Environment.from_loader(loader).resolve_type_names
    puts canonical_dump(env)
  RUBY
  out, _err, _status = Open3.capture3("ruby", "-Ispec/support", "-e", script)
  out
end
```

### 11. Test matrix

Under `spec/compat/`:

- `core_spec.rb`: core only.
- `core_stdlib_spec.rb`: core + stdlib.
- `gems_spec.rb`: core + stdlib + json / set / bigdecimal / csv / ...

The CI job `compat-test` runs all of them.

## Acceptance

- [x] All `cargo test -p librbs-core` tests are green.
- [ ] Canonical dumps for core only match pure RBS exactly.
- [ ] Canonical dumps for core + stdlib match pure RBS exactly.
- [ ] The major-gems matrix is green.
- [x] Code review confirms that the `from_loader` and `resolve_type_names`
      native paths **never call any Ruby method** (excluding the
      materialization path).
- [ ] All CI jobs are green.

## Out of scope for this milestone

- Per-Entry lazy materialization (decided in M4).
- Benchmark numbers (collected at the start of M4).
- Incremental updates (M5).
- Patches for `add_source` (added in M5 if needed).

## Pitfalls and mitigation

### Aligning canonical-dump format

If the Rust dumper and Ruby dumper are written independently they will drift.
**Define the format as a written specification** alongside the dumper
implementation. The spec authored in M3c is the source of truth; if the
Rust-side dumper is later restored (followup "Rust-side `canonical_dump`
implementation"), it must follow the same spec byte-for-byte.

Sketch of what the spec should pin down:

```
# canonical-format spec (authored in M3c, alongside spec/support/canonical_dump.rb)
- Lines separated by \n
- Namespaced names emitted in fully qualified form (e.g. ::A::B)
- Indentation: two spaces
- Collections sorted by TypeName.to_s
- ...
```

### Missed AST traversal cases

`vendor/rbs/lib/rbs/environment.rb:577-980` has a large number of AST
node-type branches. Missing one breaks the canonical dump.

Mitigation: while porting, **transcribe each Ruby line as a comment** above
the Rust counterpart. Reviewers can verify line-by-line correspondence.

### Honoring `resolve-type-names: false`

Sources carrying `# resolve-type-names: false` magic comment must **skip
resolution**. Reproduce the check from
`vendor/rbs/lib/rbs/environment.rb:534`.

### Parsing TypeName

When the native API reads the user-facing `RBS::EnvironmentLoader`, it must
extract `library:`/`version:` arguments from `add` invocations. Ivar access
on Ruby instances is straightforward in magnus.

### Injecting ivars into Ruby instances

`RBS::Environment.allocate.tap { |e| e.instance_variable_set(:@__librbs_handle, ...) }`
works, but `initialize` is skipped, so `@sources = []`, `@class_decls = {}`,
etc. **are not initialized**. Patches will overwrite them, but if `super` is
ever invoked while `@class_decls` is nil, you'll get a crash. Either add an
explicit fallback inside `ensure_materialized`, or call `send(:initialize)`
right after `allocate` to construct an empty env safely.

## Next milestone

→ [M4-decision-point.md](M4-decision-point.md)
