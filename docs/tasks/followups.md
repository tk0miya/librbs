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

### Reimplement `RBS::Environment` and `RBS::EnvironmentLoader` in Rust

- **Origin**: M3c review.
- **Where**: today — `lib/librbs/patches/environment.rb`,
  `lib/librbs/patches/environment_loader.rb`, and the magnus bridge in
  `ext/librbs/src/lib.rs` that reads the loader's ivars to build a
  Rust `Loader`. End state — pure-Rust `Environment` / `Loader`
  exposed across the magnus boundary, with the Ruby classes as thin
  facades (or removed entirely from the user's view).
- **What**: The current strategy is *patch-based*. The two upstream
  Ruby classes stay in place; we monkey-patch entry points
  (`Environment.from_loader`, soon `resolve_type_names`,
  `class_decls` etc.) to delegate into Rust. This is the right shape
  for "drop-in speedup via `require 'librbs'`" and for landing
  M3c–M3i without a re-architecture, but it carries permanent costs:
  - We pay an ivar-reading round trip on every loader-shaped input
    (`@core_root`, `@repository.@dirs`, `@libs`, `@dirs`).
  - Stringio injection and other load-time side effects have to be
    re-implemented in Rust to match the Ruby semantics
    (see `inject_stringio` in the magnus bridge).
  - Patches accumulate as we hook more entry points; behavioral
    drift between "with librbs" and "without librbs" gets harder to
    audit the more surface area we cover.
  - `@__librbs_handle` is an out-of-band attribute on `RBS::*`
    instances that anyone calling `Marshal.dump` / `dup` /
    `initialize_copy` could lose silently.
- **Ideal goal**: rewrite `Environment` and `EnvironmentLoader` as
  pure-Rust types and expose them through magnus as the canonical
  implementation (i.e. `RBS::Environment` and
  `RBS::EnvironmentLoader` themselves become magnus-wrapped Rust
  objects). The Ruby class definitions in `vendor/rbs/lib/rbs/...`
  would no longer be the authority; the patches would dissolve.
- **Required changes** (sketch — order TBD):
  - Hoist `librbs_core::Loader` / `librbs_core::Environment` to
    public Ruby classes via `magnus::wrap`, with method surfaces
    that match upstream's public API (`add`, `add_collection`,
    `each_signature`, `class_decls`, ...).
  - Replace the monkey-patches under `lib/librbs/patches/` with
    `RBS::Environment = Librbs::Environment` (and equivalent for
    Loader), guarded by a single `librbs/patches.rb` switch.
  - Decide what to do with the existing `RBS::*` classes — leave
    them dormant (still loadable for users who `without_librbs`),
    or vendor a stub that errors if invoked unpatched.
  - Audit downstream consumers (`steep`, `rubocop-rbs`, etc.) for
    code that depends on `RBS::Environment` being a plain Ruby
    object (e.g. instance variable access, `Marshal`, `inspect`
    format). Each such hook is a compatibility constraint on the
    Rust facade.
- **When**: Not a near-term blocker. M3c–M3i finish on the patch
  path. Revisit at M4 (decision point) once benchmark numbers tell
  us how much the ivar-reading round trips and patch overhead
  actually cost; if they're significant, the rewrite becomes the
  natural next step. If they're not, the patches can stay
  indefinitely and this followup becomes a nice-to-have.

### `resolve_type_names` mutates the source env's shared core state

- **Origin**: M3d review.
- **Where**: `ext/librbs/src/lib.rs` (`resolve_type_names`, lines ~228-280)
  and `lib/librbs/patches/environment.rb` (the Ruby-side patch that
  delegates into it).
- **What**: Upstream `RBS::Environment#resolve_type_names`
  (`vendor/rbs/lib/rbs/environment.rb:522-560`) is a pure function on
  `self`: it allocates `env = Environment.new`, populates it via
  `env.add_source(...)`, and returns the new env without touching the
  receiver. Our patched version returns a freshly allocated Ruby
  `RBS::Environment` object (matching the *Ruby object* identity
  contract), but
  - it mutates the underlying `librbs_core::Environment` **in place**
    through a raw `*mut` derived from the wrapped `Arc` (see the safety
    comment around `let env: &mut librbs_core::Environment = unsafe {
    &mut *env_ptr };`), and
  - it deliberately reuses the same `WrappedEnvironment` Ruby object on
    the new env (`dst.@__librbs_handle.equal?(src.@__librbs_handle)`).
  Net effect: after `dst = src.resolve_type_names`, the caller's `src`
  and `dst` share the same Arc-backed core env, and any state the
  resolver wrote (resolved decls, interned names, etc.) is observable
  through `src` too. Upstream's "self is unchanged" guarantee does not
  hold.
- **Why it's tolerable today**: every consumer in this repo (and the
  upstream call sites we vendored under `vendor/rbs/`) follows the
  pattern `env = Environment.from_loader(...).resolve_type_names` and
  immediately discards the pre-resolution env. No code reads the source
  env after calling `resolve_type_names` on it, so the shared-mutation
  is invisible in practice. The safety argument in `lib.rs:236-255`
  also relies on the strong count being 1, which the current
  `from_loader` path guarantees.
- **Risk**: A future caller (downstream gem, user script, or a new
  internal pass) that holds onto the pre-resolution env and expects it
  to stay un-resolved will see corrupted-looking state. The failure
  mode is silent — no exception, just diverging behavior between
  "with librbs" and "without librbs".
- **Required changes** — pick whichever lands first:
  - Short-term: in `resolve_type_names`, build a *new*
    `librbs_core::Environment` (cloning the inputs needed for
    resolution) instead of mutating the existing one in place, and
    wrap it in a fresh `WrappedEnvironment` for `dst.@__librbs_handle`.
    The Ruby-visible contract then matches upstream exactly.
  - Long-term: subsumed by **Reimplement `RBS::Environment` and
    `RBS::EnvironmentLoader` in Rust** below — once the core env has
    proper interior mutability (or the resolver returns a value rather
    than mutating), this hatch closes naturally.
- **When**: Before any caller starts retaining the pre-resolution env,
  and re-check at M3e (materialization adds extra Arc clones, which
  invalidates the "strong count is 1" half of the current safety
  argument). Whichever comes first.
- **Tests**: Add a spec that calls `resolve_type_names` and then asserts
  the source env still answers as un-resolved (e.g. by snapshotting a
  canonical dump of `src` before and after, or by checking that
  `src.@__librbs_handle` is *not* `equal?` to `dst.@__librbs_handle`
  once the fix lands).

### Full `Gem::Version` semantics in `librbs-core`

- **Origin**: M2 review.
- **Where**: `crates/librbs-core/src/discovery/repository.rs` (`Version`,
  `GemRBS`, `Repository`). These types are internal to `librbs-core` and
  not exposed across the Ruby boundary, but `Repository::lookup` is the
  function that selects which gem version to load — its result must
  match `Gem::Version`'s `find_best_version` for the inputs we will see.
- **What**: The current `Version` only accepts dotted numeric segments and
  rejects everything else (`1.0.0.alpha` → `None`). For `vendor/rbs/stdlib`
  this is fine — every directory there is a single `0` — so M2 acceptance
  is unaffected. Once we start looking up third-party gem versions in
  `Repository::lookup`, however, real-world inputs include prerelease and
  alphanumeric segments (e.g. `1.0.0.beta1`, `2.0.pre`, date-stamped
  builds). The same input must select the same `best_version` as
  `Gem::Version` does.
- **Required changes**:
  - Replace `segments: Vec<u64>` with a richer representation, e.g.
    `enum Segment { Num(u64), Str(String) }`.
  - Implement `correct?` equivalent (`/\A\d+(\.\d+)*([a-zA-Z][0-9a-zA-Z]*)?(\.[0-9a-zA-Z]+)*\z/`).
  - Implement `release` (drop the trailing string-prefixed tail) and
    `prerelease?`.
  - Restore the `unless version.prerelease?` exclusion in `GemRBS::load`
    so prereleases are filtered explicitly (matches Ruby behavior).
  - Comparison rules: numeric > string; same-kind segments compare with
    their natural order; `release` is used before comparing best version.
- **When**: Before the first M3+ consumer that resolves a third-party gem
  version through `Repository::lookup`.
- **Tests**: Add cases mirroring `Gem::Version` ordering for mixed
  numeric/alphabetic inputs and the exact `find_best_version` examples
  from `vendor/rbs/lib/rbs/repository.rb`.

### `DeclRef` indexing consistency between insert and lookup

- **Origin**: M2 review.
- **Where**: `crates/librbs-core/src/env/insert.rs` (writer) and the
  yet-to-be-written reader (likely `Source::decl_at(index)` or similar
  in M3).
- **What**: `DeclRef { source_index, decl_index }` is allocated in
  pre-order during `insert_rbs_source` — top-level `signature.declarations()`
  first, then recursively into `Class`/`Module` members filtered by
  `is_decl_node`. M3 will need to look the decl back up by index. If the
  reader walks the signature in a different order, or if `is_decl_node`
  expands to admit new node kinds without updating the writer, indices
  will silently drift and entries will point at the wrong AST nodes.
  Today nothing reads `decl_index`, so the bug would surface only after
  M3 starts using it.
- **Required changes (when M3 adds a reader)**:
  - Implement the reader using the same pre-order walk as
    `insert_rbs_source` (factor the traversal into a shared helper if
    possible, so writer and reader cannot disagree).
  - Add a round-trip test: build an `Environment` from a fixture file,
    iterate every entry, resolve its `DeclRef` back to a `Node`, and
    assert the node's name/kind matches the entry.
  - Consider hardening the writer so the dead `_ => {}` arm in
    `insert_decl` does not silently increment `decl_index` when an
    unexpected node slips through (`debug_assert!` on `is_decl_node`).
- **When**: As part of the M3 work that introduces the first reader of
  `DeclRef`.

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
  - `std::collections::HashMap` in `interner.rs`, `env/mod.rs`,
    `discovery/repository.rs` (predates M3a).
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
    (and `HashSet` → `FxHashSet`) across `interner.rs`, `env/mod.rs`,
    `discovery/repository.rs`. `FxHash*` are drop-in for the API surface
    we use (`new`/`default`/`insert`/`get`/`entry`/iteration).
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
