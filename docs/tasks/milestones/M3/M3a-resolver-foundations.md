# M3a: Resolver foundations (pure Rust)

## Goal

Port `RBS::Resolver::TypeNameResolver` and `RBS::Environment::UseMap` to Rust,
and define the `Resolution` side-table types. All work in this slice stays
inside `librbs-core` — no magnus bridge, no Ruby boundary changes, no AST
traversal driver.

## Prerequisites

- M2 acceptance verified (file discovery, parallel parse, entry construction).
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "Port TypeNameResolver", "Port UseMap", and "Resolution side-table".
- Study:
  - `vendor/rbs/lib/rbs/resolver/type_name_resolver.rb` (full file)
  - `vendor/rbs/lib/rbs/environment/use_map.rb` (full file)
  - `vendor/rbs/lib/rbs/namespace.rb` for the `Namespace` operations
    referenced by `resolve_namespace0`

## Scope

### `crates/librbs-core/src/resolver/`

```
resolver/
├── mod.rs
└── type_name.rs
```

`type_name.rs` ports `TypeNameResolver`:

- Struct fields use interner symbols, e.g.
  `all_names: FxHashSet<TypeNameSym>`,
  `aliases: FxHashMap<TypeNameSym, (TypeNameSym, Context)>`,
  `cache: FxHashMap<(TypeNameSym, Context), Option<TypeNameSym>>`.
- `build(env: &Environment) -> Self`.
- `resolve(&mut self, type_name: TypeNameSym, context: &Context, interner: &mut TypeNameInterner) -> Option<TypeNameSym>`.
- `resolve_namespace`, `resolve_type_name`, `resolve_head_namespace`,
  `normalize_namespace`, `resolve_namespace0` all ported faithfully — same
  control flow, same recursion shape, same cycle-detection set.
- `Context` is the existing `Vec<TypeNameSym>` from `env::entry`. Reuse the
  type; do not invent a new one.

### `crates/librbs-core/src/env/use_map.rs`

Replace the placeholder with a real port of `UseMap`:

- `UseMap::Table { known_types: FxHashSet<TypeNameSym>, children: FxHashMap<NamespaceSym, FxHashSet<TypeNameSym>> }`.
  - `populate_from(&mut self, env: &Environment)` populates `known_types`.
  - `compute_children(&mut self)` populates `children`.
- `UseMap { table: &Table, use_dirs: ..., map: FxHashMap<Sym, TypeNameSym> }`.
- `build_map(&mut self, clause)` — defer the `clause` shape to M3b
  (driver slice). For this slice, expose a method that takes the resolved
  `(name: TypeNameSym, alias: Option<Sym>)` pair and updates `map`.
- `resolve(&self, type_name: TypeNameSym, interner: &mut TypeNameInterner) -> TypeNameSym`.

The `clause` walking logic that converts a parsed `# use ...` directive into
calls to `build_map` belongs in M3b alongside the AST traversal driver.

### `crates/librbs-core/src/env/resolution.rs`

```rust
pub struct Resolution {
    pub type_name_resolutions: FxHashMap<NodeId, ResolvedRef>,
}

pub enum ResolvedRef {
    Resolved(TypeNameSym),
    Unresolved(TypeNameSym),
}
```

Pick one of:

- (a) `NodeId(pub source_index: u32, pub node_offset: u32)` keyed off
  ruby-rbs's arena offsets (preferred if a stable offset is exposed).
- (b) `NodeId(pub source_index: u32, pub serial: u32)` assigned during a
  sequential walk per source.

Either way the ID must be deterministic and computable independently per
source so that M3b's `par_iter` over sources merges cleanly.

Add a `merge` method that joins two `Resolution`s by extending the inner
hash map (panic on duplicate `NodeId` in debug builds — a duplicate signals
a NodeId allocation bug).

### `NamespaceInterner` API additions

`resolve_namespace0` walks parent namespaces and converts `TypeName` to
`Namespace`. The interner must expose:

- `parent(ns) -> Option<NamespaceSym>` — drop the last segment, preserving
  `absolute`. Returns `None` for the empty relative or root absolute.
- `is_empty(ns) -> bool` and `is_relative(ns) -> bool`.
- `to_type_name(ns) -> Option<(NamespaceSym, Sym)>` — parent + last segment;
  None for empty/root.

Closes the M2 followup
"`NamespaceInterner` API gaps and responsibility split". Add **only** the
operations the resolver needs; postpone `absolute!`, `relative!` etc. to
when an actual caller appears.

### Dependencies

Add to `crates/librbs-core/Cargo.toml`:

```toml
rustc-hash = "2"
```

(Already in the dependency tree via existing transitive crates; this just
makes the dependency direct.)

## Out of scope (deferred)

- The AST traversal that calls `resolver.resolve(...)` for every type-name
  occurrence — M3b.
- Per-source `par_iter` driver and `Resolution` merging in production code —
  M3b.
- Canonical dump — M3b.
- Anything magnus / Ruby boundary — M3c onward.

## Acceptance

- [ ] `cargo test -p librbs-core` green.
- [ ] New unit tests under `crates/librbs-core/src/resolver/type_name.rs`
      cover at minimum:
  - resolve absolute, already-known name (returns input as-is)
  - resolve unqualified name in nested context (walks outer)
  - resolve through a class alias (`aliases` lookup)
  - alias cycle detection (`visited` guard returns `false` cleanly)
  - resolve fails → returns `None`
- [ ] New unit tests for `UseMap`:
  - direct rename clause maps `Sym → TypeNameSym`
  - child lookup via `Table::children`
- [ ] No magnus / `ext/librbs` changes in this slice.
- [ ] Followup "DeclRef indexing consistency" remains open — this slice
      doesn't introduce a `DeclRef` reader yet.

## References

- `vendor/rbs/lib/rbs/resolver/type_name_resolver.rb`
- `vendor/rbs/lib/rbs/environment/use_map.rb`
- `vendor/rbs/lib/rbs/namespace.rb`
- M2 followups: "NamespaceInterner API gaps", "DeclRef indexing consistency"
