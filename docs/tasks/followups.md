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

### `NamespaceInterner::intern` allocates on every call

- **Origin**: M2 review.
- **Where**: `crates/librbs-core/src/interner.rs`.
- **What**: The HashMap key is `(Vec<Sym>, bool)`. `intern(path: &[Sym], absolute)`
  has to materialise an owned `Vec` to look up — even when the entry already
  exists. The fix landed in M2 swaps a buggy linear scan for a one-allocation
  Entry-API call, which is correct and good enough for now, but it still
  allocates per call. For ~tens of thousands of calls during environment
  construction this is fine; if a future profile shows it dominating, switch
  to `hashbrown::HashMap` and use the `raw_entry` API to look up by hash
  without owning the key.
- **When**: After we have benchmark numbers (M4) and only if this surfaces
  as a hotspot.

### `HashMap` vs `FxHashMap` inconsistency

- **Origin**: M3a review.
- **Where**: `crates/librbs-core/src/` — currently mixed:
  - `std::collections::HashMap` in `interner.rs`, `env/mod.rs`
    (predates M3a).
  - `rustc_hash::FxHashMap` / `FxHashSet` in `resolver/`, `env/use_map.rs`,
    `env/resolution.rs` (added in M3a because the spec required the
    `rustc-hash` dependency).
- **What**: Every hash key in `librbs-core` today is internally generated
  by the parser/interner (`TypeNameSym`, `Sym`, `(NamespaceSym, Sym,
  kind)`, gem-name strings). None of them are part of a HashDoS threat
  model, so the SipHash default in `std::HashMap` is paying for nothing.
  Switching the remaining sites to `FxHashMap` would be a uniform speed
  win and remove the cognitive overhead of "why this one and not that
  one". The split today is purely an artifact of when each module was
  written, not a deliberate boundary.
- **Required changes**:
  - Replace `std::collections::HashMap` with `rustc_hash::FxHashMap`
    (and `HashSet` → `FxHashSet`) across `interner.rs` and `env/mod.rs`.
    `FxHash*` are drop-in for the API surface we use
    (`new`/`default`/`insert`/`get`/`entry`/iteration).
  - Audit any newly-added module to use `FxHash*` by default unless a
    HashDoS-relevant key is introduced.
- **When**: Bundle with the M4 benchmarking pass, where we will already
  be looking at hot paths and can confirm the swap helps in practice.
  Don't do it as a standalone PR before then — the change is mechanical
  but touches enough sites to be noisy in review.

### Rust-side `canonical_dump` implementation (frozen)

- **Origin**: M3b review, refrozen during M3c.
- **Where**: `crates/librbs-core/src/canonical.rs` (does not exist).
  The Ruby-side dumper at `spec/support/canonical_dump.rb` defines
  the only canonical-dump format we currently produce; its shape is
  pinned by a top-of-file comment and is otherwise private to the
  M3 compat specs.
- **What**: The M3 series originally planned for a Rust-side dumper
  to keep the lazy-boundary contract intact under compat tests. M3c
  briefly shipped one, a separate format-spec doc, and a
  `Librbs::Native.canonical_dump` magnus bridge to drive it. We
  later reverted all three: the simpler path is to let the Ruby
  helper walk the env (post-materialization at M3h+) and accept
  that compat runs trigger materialization. Only the Ruby helper
  remains; the Rust file, the format-spec doc, and the magnus
  bridge were removed.
- **Trigger**: If the Ruby-side dumper's wall-clock time on the core
  / core+stdlib / gems compat matrix becomes a CI bottleneck, port
  the dumper back to Rust and reintroduce the magnus bridge so dumps
  do not force materialization.
- **Required changes** (when triggered):
  - Promote the format-shape comment in `spec/support/canonical_dump.rb`
    into a written cross-language spec (likely
    `docs/tasks/milestones/M3/CANONICAL_FORMAT.md`) so Rust and Ruby
    cannot drift silently.
  - Add `crates/librbs-core/src/canonical.rs` whose output is
    byte-identical to the Ruby helper for the same logical
    environment.
  - Re-export it via `pub mod canonical;` in `lib.rs`.
  - Add snapshot fixtures so format drift is caught at the Rust
    layer, not only at the magnus boundary.
  - Bridge through magnus (`Librbs::Native.canonical_dump`) so compat
    specs can call the Rust dumper instead of the Ruby helper.
- **When**: Only when the Ruby-side dumper's wall-clock time on the
  full compatibility matrix becomes a CI bottleneck. Don't pre-empt
  it.

### Wildcard `_ =>` arms in `resolver/driver.rs` defeat exhaustiveness

- **Origin**: M3b review.
- **Where**: `crates/librbs-core/src/resolver/driver.rs` — five `match`
  expressions on `ruby_rbs::node::Node` end with `_ => {}`:
  - `apply_use_directive` (use-clause dispatch)
  - `walk_declaration` (top-level decl dispatch)
  - `walk_member` (class/module/interface member dispatch)
  - `walk_type` (type-expression dispatch) ← highest risk
  - `walk_decl_index` (DeclRef lookup helper)
- **What**: The `Node` enum has 77 variants today, generated from
  `vendor/rbs/config.yml`. A `_ =>` arm silently swallows any variant
  the explicit cases don't list. If a future `ruby-rbs` release adds a
  new variant — most plausibly a new `RBS::Types::*` (which would land
  in `walk_type`) or a new `RBS::AST::Members::*` (which would land in
  `walk_member`) — our walk would skip it without recording any
  resolution. The miss surfaces only via `canonical-dump` byte
  divergence in the M3c+ compatibility tests, which is exactly the
  kind of "diff failure with no obvious cause" the M3 design tries to
  avoid.
- **Required changes** — apply per call site, in order of risk:
  - `walk_type`: replace `_ => {}` with an exhaustive match. List the
    25 type variants on the positive side; group the remaining ~50
    non-type variants into a single `Node::Class(_) | Node::Module(_)
    | ... => unreachable!("walk_type called on non-type node")` arm so
    the compiler enforces exhaustiveness and any future `ruby-rbs`
    bump that adds a variant fails to build. Same treatment for
    `walk_member`.
  - `apply_use_directive`: 2 valid clause kinds; change `_ => {}` to
    `_ => unreachable!("unknown use clause from C parser")` to surface
    parser changes immediately.
  - `walk_declaration`: callers gate on `is_decl_node` first, so the
    risk is lower; either keep `_ => {}` with a `debug_assert!` or
    promote to `unreachable!`.
  - `walk_decl_index`: catches the six member-less decl kinds
    intentionally; replace with explicit list (`Interface | TypeAlias
    | Constant | Global | ClassAlias | ModuleAlias => {}`) so the
    intent is visible.
  - Apply the same treatment to `canonical.rs::render_type` /
    `dump_member` / `dump_declaration`, which mirror these dispatches.
- **When**: Reactively, not as a prerequisite for any milestone.
  Trigger when (a) a `ruby-rbs` version bump introduces new `Node`
  variants, or (b) a canonical-dump compatibility diff surfaces with
  no obvious cause and a missed variant becomes a suspect.
- **Tests**: A meta-test that asserts the union of variants matched
  positively in each `walk_*` covers exactly the relevant slice of
  the enum is hard to write directly in Rust; the exhaustive match
  itself is the test. No additional fixture needed.

### Pre-intern UseMap rewrites so resolve becomes lock-free

- **Origin**: M3b follow-up after the read-only resolver landed.
- **Where**: `crates/librbs-core/src/env/insert.rs`,
  `crates/librbs-core/src/env/use_map.rs`,
  `crates/librbs-core/src/resolver/driver.rs`.
- **What**: As of the read-only-resolver change, the resolver and the
  driver's AST walk both run against `&TypeNameInterner` only. The one
  remaining write site during resolve is `UseMap::resolve_opt`: when a
  relative name's head segment is mapped by a `# use` directive, it
  rewrites `Bar::Baz` into `::Foo::Bar::Baz` and interns the rewritten
  namespace and `TypeNameSym`. The rewritten form is not necessarily
  literal in any source, so insert's literal-only pre-intern walk does
  not cover it; we still hold `&mut TypeNameInterner` across the call
  even though every other operation in the walk is read-only. That
  blocks per-source `par_iter` over the resolve loop.
- **Required changes**:
  - Split `env::insert::insert_rbs_source` into three passes:
    1. Walk every source's declarations and register them (today's
       behavior).
    2. After all sources are inserted, build the global
       `use_map::Table` (`populate_from` + `compute_children`).
    3. Walk every source's signatures again with a per-source
       `UseMap`, and for each reference `TypeNameNode` pre-intern
       both the literal form (today) **and** the use-rewritten form
       returned by `UseMap::resolve_opt` (new).
  - Convert `UseMap::resolve_opt` and `UseMap::resolve` to take
    `&TypeNameInterner` and use `find` / `find_join` instead of
    `intern`, returning `None` when the candidate isn't pre-interned.
    With pass 3 above, every successful rewrite is guaranteed to
    have been interned, so `None` from `resolve_opt` correctly means
    "no rewrite applies".
  - Drop the remaining `&mut env.interner` in `record_type_name`;
    the entire driver `WalkCtx` can hold `&TypeNameInterner` and
    the per-source loop can switch to `rayon::par_iter` (with a
    per-thread `Resolution` merged at the end).
- **Why deferred**: pass 2 (global `Table`) requires every source's
  declarations to be in `env.class_decls` etc. before any source's
  references can be pre-interned for wildcard-clause UseMaps. That
  forces a 1-pass-to-3-pass split of insert, which is a larger refactor
  than fits this PR. Doing it in isolation also lets us measure the
  parallelization win against a stable baseline.
- **When**: When the resolve loop becomes a measurable bottleneck on
  large inputs (likely visible on `core+stdlib+gems` once M3i lands),
  or when an external workload wants `par_iter` over sources.
- **Tests**: The existing `tests/discovery.rs::resolves_full_core_environment`
  and `tests/resolution.rs` cases cover correctness; add a test that
  resolves a fixture using `# use Foo::Bar` followed by relative
  `Bar::Baz` to lock in the pre-intern coverage. A small Criterion
  benchmark for the resolve phase would let us confirm the
  parallelization win when the switch lands.
