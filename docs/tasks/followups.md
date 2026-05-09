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

### Reimplement `RBS::Environment` and `RBS::EnvironmentLoader` as Core+Wrapper

- **Origin**: M3c review; design firmed up in pre-M4 discussion.
- **Where**: today — `lib/librbs/patches/environment.rb`,
  `lib/librbs/patches/environment_loader.rb`, and the magnus
  bridge in `ext/librbs/src/lib.rs` that reads the loader's ivars
  to build a Rust `Loader`. End state — pure-Rust
  `Librbs::EnvironmentCore` / `Librbs::EnvironmentLoaderCore`
  exposed via magnus, with thin Ruby subclasses
  `Librbs::Environment` / `Librbs::EnvironmentLoader` rebound onto
  `RBS::Environment` / `RBS::EnvironmentLoader` by constant
  reassignment.
- **What**: The current strategy is *partial patching*: we
  monkey-patch a few entry points (`from_loader`, soon
  `resolve_type_names`, `class_decls` and the five sibling decl
  hashes) on the upstream Ruby classes. This is enough for M3c–M3i
  to land but it has a structural correctness problem: any
  upstream method we forget to patch falls through to the original
  Ruby code, which reads bare ivars (`@sources`, `@class_decls`,
  ...) that the Rust handle does not populate. Concrete examples
  in `vendor/rbs/lib/rbs/environment.rb`:
  - `declarations` → `sources.flat_map(&:declarations)` (L14):
    with `*_decls` patched but `sources` not, returns `[]` while
    `class_decls` is fully populated.
  - `each_rbs_source` / `each_ruby_source` (L470-): empty iterator.
  - `buffers` → `sources.map(&:buffer)` (L998): empty array.
  - `inspect` (L993): reads ivar sizes directly, prints
    `(0 items)`.
  - `add_source` / `unload`: mutate `@sources` and the decl hashes
    in pure Ruby; the Rust handle goes out of sync.
  - `initialize_copy` (L59): dups ivars, drops `@__librbs_handle`.
  These are silent inconsistencies that surface only as
  hard-to-diagnose application bugs.
- **Ideal goal**: `RBS::Environment` and `RBS::EnvironmentLoader`
  become Ruby subclasses of magnus-wrapped Rust classes. The Rust
  layer owns all primitive state and exposes just enough accessors
  for the upstream Ruby derivative methods (`declarations`,
  `each_decl`, `inspect`, `validate_type_params`, ...) to keep
  working unchanged on top. We do not need *everything* in Rust —
  the data layer is Rust, the accessor/iteration/presentation
  layer stays Ruby.
- **Architecture**:
  - `Librbs::EnvironmentCore` (Rust, magnus `TypedData`):
    - state: `sources`, six decl tables, resolution side-table.
    - methods: `class_decls` / `interface_decls` /
      `type_alias_decls` / `constant_decls` / `class_alias_decls`
      / `global_decls` (lazy materialize), `sources` (lazy
      materialize), `add_source`, `unload`,
      `resolve_type_names(only:)`, `class_decl?`,
      `interface_name?`, and similar fast-path predicates that do
      not force materialization.
    - class methods: `from_loader`.
  - `Librbs::Environment < Librbs::EnvironmentCore` (Ruby):
    - hosts the derivative methods carried over from
      `vendor/rbs/lib/rbs/environment.rb`: `declarations`,
      `each_rbs_source`, `each_ruby_source`, `buffers`,
      `each_decl`, `each_constant`, `each_global`,
      `each_type_name`, `inspect` (rewritten to use accessors
      instead of ivars), `validate_type_params`,
      `normalize_type_name`, `normalize_module_name`,
      `class_alias?`, `module_alias?`, `class?`, `module?`,
      `absolute_type`, `subtract`, etc.
    - all derivative methods read state through the Rust
      accessors, so they stay correct as long as the accessors
      return correct data — no per-method patching required.
  - `RBS::Environment` rebound:
    ```ruby
    Librbs::Environment::ClassEntry       = RBS::Environment::ClassEntry
    Librbs::Environment::SingleEntry      = RBS::Environment::SingleEntry
    Librbs::Environment::ModuleAliasEntry = RBS::Environment::ModuleAliasEntry
    Librbs::Environment::ClassAliasEntry  = RBS::Environment::ClassAliasEntry
    Librbs::Environment::InterfaceEntry   = RBS::Environment::InterfaceEntry
    Librbs::Environment::TypeAliasEntry   = RBS::Environment::TypeAliasEntry
    Librbs::Environment::ConstantEntry    = RBS::Environment::ConstantEntry
    Librbs::Environment::GlobalEntry      = RBS::Environment::GlobalEntry

    RBS.send(:remove_const, :Environment)
    RBS::Environment = Librbs::Environment
    ```
    Constant reassignment is preferred over `prepend`-style
    patching because the latter cannot reshape allocator /
    parent-class relationships. Nested constants must be aliased
    before the rebind so that `RBS::Environment::ClassEntry` and
    friends keep resolving for downstream consumers.
  - The same Core+Wrapper pattern applies to `EnvironmentLoader`,
    with the additional choice of whether to push loader config
    (`@core_root`, `@repository`, `@libs`, `@dirs`) into Rust as
    well. Doing so eliminates the ivar-reading round trip in the
    magnus bridge and the `inject_stringio` Ruby-side detour, but
    forces the M2 follow-up "Full `Gem::Version` semantics" to
    land first because `Repository::lookup` would then run in
    Rust against real-world version strings. Decision deferred to
    the kickoff of this followup.
- **Source materialization granularity**: materialize all sources
  at once on first access, sharing the lazy boundary with the six
  decl hashes. Per-Source laziness is rejected for v1: every
  upstream method that touches `sources` iterates it in full, so
  the bookkeeping does not pay off. A two-tier split (cheap
  `Buffer` eager, AST translation lazy per Source) is left as a
  future optimization if profiling shows it.
- **Marshal**: not supported. `_dump` raises a clear `TypeError`
  to prevent silent corruption. Steep, rubocop-rbs, and typeprof
  do not call `Marshal.dump` on `Environment` (verified during M5
  investigation). Implementing `_dump` / `_load` against a
  bumpalo arena and the resolution side-table is a multi-day
  effort with no current consumer; revisit only if a real consumer
  surfaces, and at that point the most likely shape is "dump the
  inputs, replay the pipeline on load" rather than serializing
  the resolved state.
- **Required changes** (sketch — order TBD at the kickoff of this
  followup):
  - Define `Librbs::EnvironmentCore` in Rust via
    `magnus::TypedData` with the accessor surface above.
    Implement `materialize_all!` as a single entry point that
    builds the six decl Hashes and the `sources` Array.
  - Port the derivative methods of `RBS::Environment` into
    `Librbs::Environment`, rewriting any ivar reads as accessor
    calls (notably `inspect`).
  - Rebind `RBS::Environment` after aliasing nested constants.
    Drop `lib/librbs/patches/environment.rb` and the
    `@__librbs_handle` ivar.
  - Decide loader scope (full Core or `load`-only). If full Core,
    land the M2 follow-up "Full `Gem::Version` semantics in
    `librbs-core`" first, then port loader config and `add` /
    `add_collection` / repository wiring into Rust. If
    `load`-only, keep the loader Ruby-resident and only Core-ify
    the `load` entry point.
  - Audit downstream consumers (`steep`, `rubocop-rbs`,
    `typeprof`, ...) for code that depends on:
    - `RBS::Environment` being a plain Ruby object (ivar reads,
      `Marshal`, `inspect` format),
    - `klass.name == "RBS::Environment"` literal comparisons
      (the rebound class reports `"Librbs::Environment"`),
    - direct `Class#allocate` calls on `RBS::Environment`.
    Each hit is a compatibility constraint on the Rust facade.
  - Add a meta-test that asserts every public instance method of
    upstream `RBS::Environment` (and `RBS::EnvironmentLoader`)
    resolves on the rebound class, so that future RBS upstream
    bumps fail loudly when new methods land.
- **When**: Not before M4 completion. M3c–M3i finish on the
  current patch path. Revisit at M4 once benchmarks tell us how
  much the partial-patch overhead costs and how often the silent
  inconsistencies surface in practice. The architecture above is
  the agreed direction whenever this followup is taken on; the
  open question is timing, not shape.

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
