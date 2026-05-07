# M3e: Materialization (AST → `RBS::AST::*`)

## Goal

Implement `materialize_all`: convert the Rust-side environment into genuine
`RBS::AST::*` declarations and `RBS::Types::*` instances and populate the
six `*_decls` Hashes on the patched `RBS::Environment`. This is the slice
described in the parent doc as "the bulk of M3 implementation work" and
the only path that can legitimately call Ruby methods.

## Prerequisites

- M3a + M3b + M3c + M3d merged.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "AST → Ruby conversion", "materialize_all flow", and
  "Pitfalls / Byte ↔ character offset bridge for `RBS::Location`".
- Resolve the M2 followup
  "Byte ↔ character offset bridge for `RBS::Location`" — pick approach (A)
  (extend ruby-rbs) or (B) (Rust-side conversion). Decision recorded in
  the PR description.

## Scope

### `ext/librbs/src/materialize/`

```
materialize/
├── mod.rs
├── decl.rs       // RBS::AST::Declarations::*
├── member.rs     // RBS::AST::Members::*
├── type_.rs      // RBS::Types::*
├── method_type.rs
├── type_param.rs
├── location.rs   // byte → char conversion + RBS::Location.new
└── type_name.rs  // RBS::TypeName.new from TypeNameSym, honoring resolution
```

For each AST node category, define a function that takes:

- the Rust AST handle (or an arena ID),
- the surrounding context (containing `Buffer`, `Resolution`, interner),

and returns a magnus `Value` constructed via `RBS::AST::*.new(...)`.

Strict ordering invariant: every `new(...)` argument must match the
upstream Ruby class signature exactly. Diff the constructors against
`vendor/rbs/lib/rbs/ast/declarations.rb`,
`vendor/rbs/lib/rbs/ast/members.rb`, `vendor/rbs/lib/rbs/types.rb`,
`vendor/rbs/lib/rbs/method_type.rb`, `vendor/rbs/lib/rbs/ast/type_param.rb`.

### `ext/librbs/src/lib.rs`

Add `materialize_all`:

```rust
fn materialize_all(env: Value) -> Result<(), Error>;
```

Flow:

1. If `@__librbs_materialized` is `true`, return `Ok(())`.
2. Extract `Arc<Environment>` and (optional) `Arc<Resolution>` from ivars.
3. For each entry hash, build a Ruby `Hash` keyed by `RBS::TypeName`,
   valued by the matching `RBS::Environment::*Entry`.
4. Set `@class_decls`, `@interface_decls`, `@type_alias_decls`,
   `@constant_decls`, `@class_alias_decls`, `@global_decls`.
5. Set `@__librbs_materialized = true`.

Type-name lookup during materialization (per parent doc):

- `Some(Resolved(sym))` → construct `RBS::TypeName.new(...)` from the
  resolved symbol; mark absolute.
- `Some(Unresolved(sym))` → construct from the original symbol; **do not**
  call `absolute!`.
- `None` (entry missing from `Resolution`) → use the AST's original name.

### Patches

```ruby
# lib/librbs/patches/environment.rb (extending M3d)
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

The `instance_variable_defined?` guard is required because some env
instances are constructed via the pure path (`RBS::Environment.new`) and
have no Rust handle. For those, fall through to `super`.

### `RBS::Location` byte/char fix

If approach (B) is chosen, add a `Buffer::byte_to_char_offset(b: usize)`
helper with caching. Add multi-byte regression tests:

```rbs
# fixtures/multibyte.rbs
# 日本語コメント
class Foo
end
```

The materialized `RBS::Location` for `Foo` must have line/column matching
what pure RBS produces.

### Tests

- `spec/unit/materialize_spec.rb`: build env → materialize → inspect
  `class_decls` keys, walk a class entry's `decls.first.members.first` and
  assert it's an instance of `RBS::AST::Members::*`.
- `spec/compat/object_spec.rb`: pick a small set of stdlib types and
  assert `env.class_decls[name]` is `==` (deep) to the pure-RBS env's
  entry. Use the existing `==` definitions on `RBS::AST::*`.

## Out of scope (deferred)

- core+stdlib / gems compat matrix — M3f.
- Per-Entry lazy materialization — M4 (decision point).
- Performance benchmarks — M4.

## Acceptance

- [ ] `Librbs::Native.materialize_all(env)` populates the six `*_decls`
      ivars with real `RBS::AST::*` instances.
- [ ] Re-entrancy: calling `materialize_all` twice is a no-op.
- [ ] Patched accessors auto-trigger materialization and return the same
      object on repeat calls.
- [ ] M2 followup "Byte ↔ character offset bridge" closed: multi-byte
      regression tests pass.
- [ ] Per-entry equality spec passes against a curated subset of stdlib
      types (full matrix is M3f).
- [ ] CI green.

## References

- `vendor/rbs/lib/rbs/ast/declarations.rb`
- `vendor/rbs/lib/rbs/ast/members.rb`
- `vendor/rbs/lib/rbs/ast/type_param.rb`
- `vendor/rbs/lib/rbs/types.rb`
- `vendor/rbs/lib/rbs/method_type.rb`
- M2 followup: "Byte ↔ character offset bridge for `RBS::Location`"
