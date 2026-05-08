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

### Rust-side `canonical_dump` implementation

- **Origin**: M3b review.
- **Where**: `crates/librbs-core/src/canonical.rs` (deleted at the end
  of M3b). The canonical-dump format spec lives in M3c, alongside the
  Ruby-side dumper that consumes it.
- **What**: M3b initially shipped a Rust-side `canonical_dump` so the
  M3 lazy-boundary contract held even during compatibility tests
  (Ruby-side dumping would force materialization). On review we
  decided the simpler path is to defer the Rust dumper and let M3c's
  Ruby-side `canonical_dump` walk the materialized env, accepting
  that compatibility runs trigger materialization. The Rust file and
  its standalone format-spec doc were removed; M3c rebuilds the spec
  as part of writing the Ruby dumper.
- **Trigger**: If the Ruby-side `canonical_dump` becomes too slow on
  the core / core+stdlib / gems compatibility matrix to be practical
  in CI, port the dumper back to Rust and call it across the magnus
  boundary so the dump runs without materializing.
- **Required changes** (when triggered):
  - Restore `crates/librbs-core/src/canonical.rs` from
    `git show 0d449d6:crates/librbs-core/src/canonical.rs` (the
    initial M3b implementation) or the dedup'd version in
    `b313b1c`.
  - Re-export it via `pub mod canonical;` in `lib.rs`.
  - Re-add the snapshot fixtures from the M3b version of
    `tests/resolution.rs` (`canonical_dump_simple_fixture_is_stable`
    et al.) so the format does not drift silently.
  - Bridge through magnus (`Librbs::Native.canonical_dump`) so M3c+
    compatibility specs can call the Rust dumper instead of the
    Ruby helper.
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

### Byte ↔ character offset bridge for `RBS::Location`

- **Origin**: M2 review of `Buffer`.
- **Where**: M3 materialization path that constructs `RBS::Location.new(buffer, start_pos, end_pos)`.
- **What**: `ruby-rbs`'s Rust binding only exposes byte offsets
  (`RBSLocationRange::start()` returns `start_byte`). `RBS::Buffer#pos_to_loc`
  in Ruby, however, indexes by **character offset** (it splits via
  `String#lines` and measures via `String#size`). Passing byte offsets into
  `RBS::Location` gives wrong line/column results for any source containing
  multi-byte characters (e.g. comments with Japanese text). For ASCII-only
  RBS files there is no observable difference; for general inputs the LSP /
  editor surfaces and user-facing error output will be off by however many
  multi-byte chars precede the position.
- **Required changes** — pick one:
  - (A) Extend the `ruby-rbs` binding (or reach into the C struct directly)
    to expose `start_character_offset` / `end_character_offset`, which the C
    parser already maintains. This is the cleanest path.
  - (B) Convert byte → character offset on the Rust side at materialization
    time using `content[..byte_pos].chars().count()`. Cache per-source if
    profiling shows hotspots.
- **Note**: Independent of how line/column is computed — even if line/col
  is computed lazily by `RBS::Buffer` on the Ruby side, the position values
  passed in must already be character offsets. Buffer line tracking on the
  Rust side is *not* needed for this; only the offset conversion is.
- **When**: As part of the M3 materialization implementation. Add
  multi-byte regression tests at the same time.
