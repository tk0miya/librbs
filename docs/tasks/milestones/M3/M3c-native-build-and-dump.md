# M3c: Native `build_environment` + canonical-dump format and core compat spec

## Goal

Cross the magnus boundary for the first time in M3. Expose
`build_environment` to Ruby, write the canonical-dump format spec
*and* the Ruby-side `canonical_dump` helper that implements it, and
lock in the first acceptance checkbox: **canonical dump for core only
matches pure RBS**.

The canonical-dump format spec was originally drafted in M3b. It has
been moved here so spec and the implementation that follows it land
together; see the followup "Rust-side `canonical_dump`
implementation" for the deferred Rust port.

## Prerequisites

- M3a + M3b merged.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "Native API", "Patch layer" (only the `from_loader` portion is
  relevant in this slice), and "Pitfalls / Injecting ivars into Ruby
  instances".

## Scope

### `ext/librbs/src/lib.rs`

Add two singleton methods on `Librbs::Native`:

```rust
fn build_environment(loader: Value) -> Result<Value, Error>;
fn canonical_dump_native(env: Value) -> Result<String, Error>;
```

`build_environment`:

1. Read `core_root`, `repository`, `libs`, `dirs` from the supplied
   `RBS::EnvironmentLoader` via ivar access (`@core_path`, `@repository`,
   `@libs`, `@dirs`). Resolve `RBS::EnvironmentLoader::DEFAULT_CORE_ROOT`
   when `@core_path` is the default sentinel.
2. Repackage into `librbs_core::Loader` (already exists).
3. Call `Environment::from_loader(&mut loader)`.
4. `RBS::Environment.allocate` to obtain an empty Ruby instance, then call
   `send(:initialize)` so `@class_decls = {}` etc. are initialized (avoids
   the "ivars not initialized when super is called" pitfall).
5. Box the `Arc<librbs_core::Environment>` in a magnus `TypedData`
   wrapper and assign to `@__librbs_handle`.
6. Return the `RBS::Environment` instance.

`canonical_dump_native`:

1. Extract the `Arc<Environment>` from `@__librbs_handle`.
2. Look up `@__librbs_resolution` ivar; if present extract the
   `Arc<Resolution>` (introduced fully in M3d, but support being absent
   here — pass `None` to `canonical_dump`).
3. Call `librbs_core::canonical::canonical_dump(env, resolution.as_deref())`
   and return the resulting `String` to Ruby.

This call path **must not invoke any Ruby method** beyond the ivar reads —
that's the M3 invariant. Code review at the end of M3d verifies this for
the resolver path; this slice already establishes the discipline.

### TypedData wrappers

Define a `WrappedEnvironment(Arc<librbs_core::Environment>)` that magnus
treats as `TypedData`. Use a free-function `mark`/`free` pair following the
magnus 0.8 patterns. Ensure `Send + Sync` because magnus requires it on
TypedData.

Likewise define `WrappedResolution(Arc<Resolution>)` for the M3d ivar even
though M3c does not yet write it — having the wrapper in place avoids
churn.

### `lib/librbs/patches/environment_loader.rb`

```ruby
module Librbs
  module Patches
    module EnvironmentLoader
      module ClassMethods
        # no overrides yet; reserved for future use
      end
    end
  end
end
```

### `lib/librbs/patches/environment.rb`

```ruby
module Librbs
  module Patches
    module Environment
      module ClassMethods
        def from_loader(loader)
          Librbs::Native.build_environment(loader)
        end
      end
    end
  end
end

RBS::Environment.singleton_class.prepend(Librbs::Patches::Environment::ClassMethods)
```

`resolve_type_names` patch is deliberately **not** added in this slice —
that's M3d.

Update `lib/librbs/patches.rb` to require both patch files.

### Canonical-dump format spec + Ruby-side helper

Author the canonical-dump format specification (originally drafted
for M3b, then deferred) alongside the Ruby implementation that
follows it. Both land in this slice so they cannot drift.

Suggested layout:

```
docs/tasks/milestones/M3/CANONICAL_FORMAT.md   # the format spec
spec/support/canonical_dump.rb                  # Ruby implementation
```

The Ruby helper walks a real `RBS::Environment` and must:

- Iterate `env.class_decls.keys.sort_by(&:to_s)` and emit each entry.
- For each declaration, walk members and types in a deterministic
  order pinned by the spec.
- Emit type names via `TypeName#to_s` (already absolute after
  `resolve_type_names`).

If the Rust-side dumper is later restored (followup "Rust-side
`canonical_dump` implementation"), it must produce byte-identical
output to this Ruby helper for the same environment.

### `spec/compat/canonical_dump_core_spec.rb`

```ruby
RSpec.describe "canonical_dump compatibility (core)" do
  it "matches between Rust and Ruby for unresolved core" do
    loader = RBS::EnvironmentLoader.new
    env = RBS::Environment.from_loader(loader)  # native path
    rust_dump = Librbs::Native.canonical_dump(env)

    pure_env = without_librbs do
      RBS::Environment.from_loader(RBS::EnvironmentLoader.new)
    end
    ruby_dump = canonical_dump(pure_env)

    expect(rust_dump).to eq(ruby_dump)
  end
end
```

`without_librbs` runs a fresh ruby subprocess (per the parent doc's
"`without_librbs` ... fresh Ruby subprocess" note) so monkey-patches don't
leak.

> **Resolution caveat**: `resolve_type_names` is not patched in this slice,
> so both `env`s above are unresolved. The compat spec passes by emitting
> *original* type names from both sides. The "resolved core" diff
> (acceptance item "canonical dumps for core only match") is closed in
> M3d once `resolve_type_names` is bridged.

## Out of scope (deferred)

- `resolve_type_names` magnus bridge — M3d.
- `materialize_all` / patches that call `ensure_materialized` — M3e.
- core+stdlib and gems matrices — M3f.

## Acceptance

- [ ] `Librbs::Native.build_environment(RBS::EnvironmentLoader.new)` returns
      a real `RBS::Environment` whose `@__librbs_handle` is set.
- [ ] `Librbs::Native.canonical_dump(env)` returns a deterministic string.
- [ ] `RBS::Environment.from_loader` patched to delegate to
      `build_environment`.
- [ ] `spec/compat/canonical_dump_core_spec.rb` green for the
      **unresolved** core: Rust dump matches the Ruby dumper output on a
      pure-RBS env.
- [ ] No magnus call from the native dump path executes any Ruby method
      besides ivar reads (peer review).
- [ ] `bundle exec rspec` and `cargo test` both green in CI.

## References

- `vendor/rbs/lib/rbs/environment.rb` (`from_loader`)
- `vendor/rbs/lib/rbs/environment_loader.rb` (ivar shape)
- magnus 0.8 `TypedData` patterns
