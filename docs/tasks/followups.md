# Followups

Items deferred from the milestone in which they were noticed. Not blocking
the milestone's acceptance, but must be addressed before the next consumer
relies on the missing behavior.

## Scoping rule

When a Rust type does not cross the Ruby boundary (i.e. it is internal to
`librbs-core` and never appears in `RBS::*` patches), behavioral differences
from the Ruby original are absorbable as long as no Rust caller depends on
the Ruby semantics. Such cases do not need to be tracked here — fix or
remove them locally. Only divergences that affect a real or imminent caller
belong in this list.

## Open

### Complete materialize-trigger coverage on `RBS::Environment`

- **Origin**: M3c review; reframed post-M4. Originally tracked as
  "Reimplement `RBS::Environment` and `RBS::EnvironmentLoader` as
  Core+Wrapper" because the partial-patch strategy was thought to
  be structurally unsound. M3e's `add_source`-based materialization
  changed that: once `materialize_all` runs, the upstream ivars
  (`@sources`, `@class_decls`, ...) are populated bit-for-bit
  identically to a pure-Ruby load. The remaining work is therefore
  not a full rewrite — it is closing the trigger coverage on every
  state-reading method, plus a meta-test that prevents the next
  upstream bump from silently reopening the gap.
- **Where**: `lib/librbs/patches/environment.rb` and `test/rbs/`
  (or `spec/`). The companion work of moving the `from_loader`
  patch off `RBS::Environment` and onto `RBS::EnvironmentLoader`
  is in progress separately and is *not* part of this followup.
- **What**: today the patch covers `class_decls`, `interface_decls`,
  `type_alias_decls`, `constant_decls`, `class_alias_decls`,
  `global_decls`, `sources`, `declarations`, `each_rbs_source`,
  and `each_ruby_source`. Predicates and normalizers
  (`interface_name?`, `class_decl?`, `normalize_type_name`, ...)
  route through these accessors and inherit the trigger
  transitively. Known holes:
  - **`inspect`** (`vendor/rbs/lib/rbs/environment.rb:993`) reads
    ivar sizes directly (`@class_decls.size`, ...) and bypasses
    the patched accessors. `pp env` before any other access
    prints `(0 items)` for every category.
  - **`initialize_copy`** (L59) dups the six decl Hashes and
    `@sources` but drops `@__librbs_handle` and
    `@__librbs_materialized`. If `dup` runs before any
    materializing read, the dup permanently sees empty ivars.
  - **`add_source` / `unload`** mutate the Ruby ivars directly.
    After `materialize_all` has run this is fine — the Rust
    handle is never re-read once materialization is complete —
    but before materialization, the user mutation lands on empty
    ivars and is overwritten when materialization eventually
    runs.
  - **`buffers`** (L998) routes through the patched `sources`
    method and is therefore safe. Listed here only to record
    that we audited it.
- **Required changes**:
  - Add `inspect` to the `ensure_materialized; super()` list in
    `lib/librbs/patches/environment.rb`.
  - Add `initialize_copy` so the source env is materialized
    before its ivars are dup'd. The `@__librbs_handle` /
    `@__librbs_materialized` ivars are intentionally not copied
    — the post-materialize Ruby ivars are the source of truth on
    the dup.
  - Add `add_source` / `unload` so each one materializes the
    existing Rust state before applying the user's mutation. Net
    effect: the Rust handle becomes write-once / read-once
    (consumed by the first materialize call), and all subsequent
    state lives in Ruby ivars.
  - Add a meta-test that enumerates every public instance method
    of upstream `RBS::Environment` and asserts each is either
    (a) explicitly patched in `Librbs::Patches::Environment`, or
    (b) reads state only through patched accessors (no direct
    ivar reads, no `instance_variable_get`). A future upstream
    bump that introduces a new ivar-reading method then fails
    the meta-test instead of silently shipping a
    `(0 items)`-style bug.
- **When**: Now. Independent of the `resolve_type_names`
  followup below.
- **Tests**: The meta-test is the primary regression guard. Add
  targeted unit specs for the specific behaviours: `pp env`
  reports correct sizes both pre- and post-materialize;
  `env.dup.class_decls` matches `env.class_decls`;
  `env.add_source(s)` called before any other access yields an
  env whose `class_decls` covers both the Rust-side sources and
  `s`.
