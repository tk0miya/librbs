# M3d: `resolve_type_names` magnus bridge

## Goal

Bridge `resolve_type_names` to Rust so that the resolution side-table
computed by M3b is reachable through the patched `RBS::Environment`. Close
the acceptance item "canonical dumps for core only match pure RBS exactly"
in its **resolved** form.

## Prerequisites

- M3a + M3b + M3c merged.
- `WrappedResolution` TypedData wrapper exists from M3c.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "Native API > resolve_type_names flow" and "Pitfalls / Honoring
  `resolve-type-names: false`".

## Scope

### `ext/librbs/src/lib.rs`

Add:

```rust
fn resolve_type_names(env: Value, only: Option<Value>) -> Result<Value, Error>;
```

Flow:

1. Extract `Arc<librbs_core::Environment>` from `@__librbs_handle`.
2. Convert `only:` (a `Set<TypeName>` or `nil`) into a Rust
   `Option<FxHashSet<TypeNameSym>>`. Missing → resolve everything.
3. Call `librbs_core::resolver::driver::resolve(&env)` (or a `resolve_only`
   variant when `only` is present). The driver already honors
   `# resolve-type-names: false` per M3b.
4. `RBS::Environment.allocate.tap { send(:initialize) }`.
5. Set `@__librbs_handle` to the **same** `Arc<Environment>` (sources /
   entries are shared — only the resolution differs).
6. Set `@__librbs_resolution` to a `WrappedResolution(Arc::new(resolution))`.
7. Return the new `RBS::Environment`.

### Patches

```ruby
# lib/librbs/patches/environment.rb (extending M3c)
def resolve_type_names(only: nil)
  Librbs::Native.resolve_type_names(self, only)
end
```

The patch lives on the instance side and is `prepend`ed onto
`RBS::Environment`.

### `only:` semantics

Match `vendor/rbs/lib/rbs/environment.rb:resolve_type_names(only:)`:

- `only` is a `Set[TypeName]` of names to resolve; everything else is
  copied as-is from the source env.
- The driver must accept the set, convert each `RBS::TypeName` to a
  `TypeNameSym` via the interner, and skip declarations whose entry name
  isn't in the set.

### Compat spec

Update `spec/compat/canonical_dump_core_spec.rb` from M3c so that **both**
sides call `resolve_type_names` and the diff is taken on the resolved
canonical dump. Keep the unresolved variant under a separate `it` block —
both must pass.

```ruby
it "matches Rust vs Ruby for resolved core" do
  loader = RBS::EnvironmentLoader.new
  env = RBS::Environment.from_loader(loader).resolve_type_names
  rust_dump = Librbs::Native.canonical_dump(env)

  pure_env = without_librbs do
    RBS::Environment.from_loader(RBS::EnvironmentLoader.new).resolve_type_names
  end
  ruby_dump = canonical_dump(pure_env)

  expect(rust_dump).to eq(ruby_dump)
end
```

### Native-purity audit

End the slice with a peer review of every code path reachable from
`build_environment` and `resolve_type_names`. The reviewer must confirm
that no `rb_funcall` / magnus `funcall` / `eval` / Ruby method dispatch
happens beyond the documented ivar reads. The audit result is recorded in
the PR description (no separate doc).

This satisfies the parent acceptance item:

> Code review confirms that the `from_loader` and `resolve_type_names`
> native paths never call any Ruby method (excluding the materialization
> path).

## Out of scope (deferred)

- `materialize_all` and AST → Ruby — M3e (plumbing) / M3f (types) /
  M3g (members + method types) / M3h (decls + entries + cut-over).
- core+stdlib / gems matrices — M3i. (Core only is enough for this slice;
  stdlib often resolves identically but the matrix expansion is M3i.)

## Acceptance

- [x] `Librbs::Native.resolve_type_names(env, only)` returns a fresh
      `RBS::Environment` whose `@__librbs_handle` is shared with the input
      and whose `@__librbs_resolution` is a `WrappedResolution`.
- [x] `RBS::Environment#resolve_type_names` patched to call the native API.
- [x] `only:` semantics honored — unit test covers resolving a single
      type name only.
- [x] `spec/compat/canonical_dump_core_spec.rb` green for both unresolved
      and resolved variants.
- [x] Native-purity audit completed and noted in the PR description.
- [x] `bundle exec rspec` and `cargo test` green in CI.

## References

- `vendor/rbs/lib/rbs/environment.rb:500-560`
