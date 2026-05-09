# M3e: Materialization plumbing (location, type_name)

## Goal

Stand up the foundations that every later materialization slice depends
on, **without populating any of the six `*_decls` Hashes yet**. By the
end of this slice we have:

- `ext/librbs/src/materialize/` exists with `mod.rs`, `location.rs`,
  `type_name.rs`,
- a `MaterializeCtx` that bundles per-source state (cached
  `RBS::Buffer`, `Resolution` lookup, per-decl resolution cursor),
- correct byte → character offset translation backed by the upstream
  `RBS::Location` API,
- `Librbs::Native.materialize_all` is **declared and registered** but
  no-ops (returns `nil`) — the `*_decls` accessors stay unpatched, so
  M3a–M3d functionality is unchanged.

### Note on the resolution side-table

While implementing M3e the original `(source_index, serial)`-keyed
`NodeId` scheme (sketched in M3a/M3b) was replaced by per-`DeclRef`
storage:

```rust
pub struct Resolution {
    entries: FxHashMap<DeclRef, Vec<ResolvedRef>>,
}
```

(`record(decl_ref, resolved)` writes, `get(decl_ref)` reads.)

The materializer iterates `env.*_decls`, looks each entry's `DeclRef`
up via `lookup_decl`, and walks the AST subtree pulling `ResolvedRef`s
from `Resolution::get(decl_ref)` in pre-order. Walking the source again
from the start to reproduce a global serial is unnecessary — `*_decls`
is already organized by entry, so resolutions live alongside that
organization.

### Walker drift

The M3f-h materializer writes its own AST walker that must visit
type-name occurrences in the same pre-order as
`resolver::driver::record_type_name`. Drift surfaces as a
canonical-dump compat failure in M3h. The broader concern of
"compile-time exhaustive matching catches missed AST variants" is
tracked under followups.md "Wildcard `_ =>` arms in
`resolver/driver.rs` defeat exhaustiveness".

Splitting the original M3e into M3e/M3f/M3g/M3h is the result of the
PR-#10 retrospective: a single slice that covers every AST variant
ended up landing as a Ruby-side re-parse shortcut that bypassed
M3a–M3d entirely. The four slices now stage the per-node Rust → Ruby
translation in pieces small enough to review and unit-test
independently, with the cut-over happening only at M3h.

## Prerequisites

- M3a + M3b + M3c + M3d merged.
- The in-flight `ruby-rbs` parser rewrite that exposes character
  offsets directly on `RBSLocationRange` (`start_char` / `end_char`)
  has landed. M3e relies on that: byte-offset accessors are not
  enough, and a Rust-side byte→char converter is no longer planned.
  See followups.md "Byte ↔ character offset bridge for `RBS::Location`"
  for the historical context — the bridge is delivered by the parser
  rewrite, not by this slice.
- Read the parent
  [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "AST → Ruby conversion" and "materialize_all flow".

## Scope

### `ext/librbs/src/materialize/mod.rs`

`MaterializeCtx` carries everything per-source materialization needs:

```rust
pub struct MaterializeCtx<'a> {
    pub ruby: &'a Ruby,
    pub env: &'a librbs_core::Environment,
    pub resolution: Option<&'a Resolution>,
    pub source_index: u32,
    /// Cached RBS::Buffer for the current source. One Buffer per
    /// source so all locations share the same backing object —
    /// upstream RBS uses Buffer identity in some equality checks.
    buffer: Option<Value>,
    /// Slice of `ResolvedRef`s for the decl currently being walked.
    /// Set via `enter_decl(decl_ref)` before walking the decl's AST
    /// subtree; `pull_resolution()` consumes from it in pre-order.
    /// `None` when the env was never resolved or when the decl was
    /// skipped during resolution (`only:` / magic comment).
    current_resolutions: Option<&'a [ResolvedRef]>,
    cursor: usize,
    /// Pre-resolved Ruby class refs (`RBS::TypeName`, `RBS::Buffer`,
    /// `RBS::Location`, `Pathname`, …) so we don't `eval` per call.
    pub classes: ClassRefs,
}
```

A `ClassRefs` struct caches every `magnus::RClass` we'll need so the
type/member/decl slices can look classes up by field rather than
repeating `ruby.eval("RBS::…")` on every node.

### `ext/librbs/src/materialize/location.rs`

- `make_location(ctx, range) -> Value` constructing
  `RBS::Location.new(buffer, range.start_char(), range.end_char())`
  with the cached buffer. Character offsets come straight from the
  parser; no byte→char conversion happens on the Rust side.
- Sub-location helpers:
  `add_required_child(loc, name, range)` and
  `add_optional_child(loc, name, range_or_nil)` — calls upstream
  `Location#add_required_child` / `#add_optional_child` with
  character-offset ranges per the `RBS::Location` API.

### `ext/librbs/src/materialize/type_name.rs`

- `materialize_type_name(ctx, raw: TypeNameSym) -> Value` — given an
  interned name, build `RBS::TypeName.new(namespace:, name:)` and
  call `absolute!` if the namespace is absolute. Reads the
  `TypeNameInterner` for namespace path + leaf name + kind tag.
- `materialize_resolved_type_name(ctx, raw: TypeNameSym) -> Value` —
  the variant that consumes `Resolution`. Pulls the next `ResolvedRef`
  from `ctx.pull_resolution()`:
  - `None` (env unresolved, decl skipped, or cursor exhausted) → use
    `raw` as-is, no `absolute!`.
  - `Resolved(sym)` → build from `sym`, mark `absolute!`.
  - `Unresolved(sym)` → build from `sym`, **do not** mark
    `absolute!` (matches upstream's `|| type_name` behavior).
  The caller is responsible for invoking `ctx.enter_decl(decl_ref)`
  before walking each decl's subtree.

### Test entry points (temporary)

Expose private singleton methods under `Librbs::Native` so each layer
can be unit-tested without the rest of the pipeline:

```rust
fn _materialize_first_class_name(env: Value) -> Result<Value, Error>;
```

Reads the env's first source, walks to the first declaration, returns
the materialized `RBS::TypeName`. Removed at M3h alongside the rest of
the test entries.

### `ext/librbs/src/lib.rs`

- Register an empty `materialize_all(env)` that returns `nil` after
  flipping `@__librbs_materialized` to `true` (so re-entry tests
  written ahead of M3h still see the no-op semantics).
- Wire the new `_materialize_first_class_name` test entry.

### Tests

- `spec/unit/materialize_location_spec.rb`: ASCII source, multi-byte
  source — assert `start_line` / `start_column` of the first decl
  match pure-RBS values. The multi-byte case is a regression guard
  against accidentally re-introducing byte arithmetic anywhere in
  `make_location` / sub-location helpers (the parser rewrite
  removes the conversion need, but it's cheap to keep verifying).
- `spec/unit/materialize_type_name_spec.rb`: four cases — Resolved
  (absolute), Unresolved (relative), no-resolution env, and a
  multi-segment name like `::Foo::Bar`.
- The existing M3d compat specs must remain green; the canonical
  dump compat specs stay `pending` (they pend on M3h).

## Out of scope (deferred)

- `RBS::Types::*` materialization → M3f.
- `RBS::AST::Members::*` and `RBS::MethodType` → M3g.
- `RBS::AST::Declarations::*`, entry construction, accessor
  patches, `materialize_all` cut-over → M3h.
- Per-Entry lazy materialization → M4.
- Compat matrix expansion → M3i.

## Acceptance

- [x] `materialize/{mod,location,type_name}.rs` exist and compile.
- [x] `Resolution` is keyed by `DeclRef` with per-decl
      `Vec<ResolvedRef>`; `MaterializeCtx` exposes
      `enter_decl(decl_ref)` + `pull_resolution()` so the M3f-h
      materializer can consume resolutions in pre-order while walking
      a decl's AST subtree.
- [x] Multi-byte regression fixture: pure RBS' `RBS::Location` for
      `Foo` in `spec/fixtures/multibyte.rbs` matches the location the
      M3e helper produces.
- [x] `Librbs::Native.materialize_all(env)` exists and is idempotent
      (no-op for now).
- [x] All M3a–M3d specs green; canonical-dump compat specs remain
      `pending` (unblocked at M3h).

## References

- `vendor/rbs/lib/rbs/location.rb`
- `vendor/rbs/lib/rbs/buffer.rb`
- `vendor/rbs/lib/rbs/type_name.rb`
- `crates/librbs-core/src/resolver/driver.rs` (per-DeclRef recording)
- `crates/librbs-core/src/env/resolution.rs`
- M2 followup: "Byte ↔ character offset bridge for `RBS::Location`"
- M3b followup: "Wildcard `_ =>` arms in `resolver/driver.rs` defeat
  exhaustiveness" — broader compile-time guard for AST coverage
