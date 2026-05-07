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

### `NamespaceInterner` API gaps and responsibility split

- **Origin**: M2 review.
- **Where**: `crates/librbs-core/src/interner.rs`.
- **What**: `NamespaceInterner` currently mixes two responsibilities — an
  ID registry (`map`, `rev`, `intern_owned`, `lookup`) and value-level
  operations on namespaces (`append`, `join`). This is fine for now and
  matches the usual Rust interning idiom (e.g. `string-interner`'s
  `get_or_intern`). Missing operations to add as M3 needs them: Ruby's
  `RBS::Namespace` has `parent`, `to_type_name`, `empty?`, `relative?`,
  `absolute!`, `relative!`, `==` via path/absolute. `parent` will
  definitely be needed by the M3 `TypeNameResolver` port
  (`resolve_namespace0` walks parent namespaces). Add operations as they
  are needed, and at that point reconsider whether to keep them on the
  interner or move them onto a thin `Namespace` value or `NamespaceSym`
  extension.
- **When**: As part of M3 when porting `TypeNameResolver` /
  `resolve_namespace0`. Don't pre-implement operations we don't have a
  caller for.

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
