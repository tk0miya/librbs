# M3j: Writer-based per-decl test oracle

## Goal

Tighten the two materialization-correctness specs that survive M3h —
`spec/unit/materialize_spec.rb` and `spec/compat/object_spec.rb` — by
swapping their oracles for `RBS::Writer` text comparison. Keep the
bulk-parity `canonical_dump` matrix from M3i untouched. Keep
`spec/unit/materialize_location_spec.rb`'s replacement (or whatever
M3h promotes location coverage into) as the sole owner of
location/buffer-identity assertions.

This is a **test-only** slice. No production-code, native-bridge, or
patch-layer changes. The acceptance bar is: every regression that the
M3h+M3i oracles caught is still caught, expressed in fewer lines and
with stronger guarantees on user-visible RBS syntax.

## Prerequisites

- M3a + M3b + M3c + M3d + M3e + M3f + M3g + M3h + M3i merged.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  section "Compatibility tests" for the canonical-dump contract this
  slice does **not** replace, only complements.
- Read `vendor/rbs/lib/rbs/writer.rb` end-to-end. The non-`preserve!`
  code path is the one this slice relies on; understand which AST
  fields it touches and which it ignores.
- Re-read M3h's "Cleanup" and "Tests" sections — M3j picks up exactly
  where M3h's cleanup leaves off. Do not re-list specs M3h already
  deleted.

## Background

By the time M3j starts, M3h has already removed the per-shape C test
entries (`_materialize_first_*`) and the M3e/M3f unit specs that
depended on them. What survives, and is the actual target of this
slice:

1. `spec/unit/materialize_spec.rb` (M3h) — asserts that each
   `*_decls` Hash is populated with the right entry/decl/member
   classes via `is_a?` chains, plus re-entrancy and the pure-Ruby
   `RBS::Environment.new` fallback.
2. `spec/compat/object_spec.rb` (M3h) — per-name `each_decl.first
   .to_json` parity against a pure-RBS subprocess for a curated core
   set, both unresolved and resolved.
3. `spec/support/canonical_dump.rb` + `spec/compat/core_spec.rb` /
   `core_stdlib_spec.rb` / `gems_spec.rb` (M3i) — bulk
   environment-level parity. Not in scope for replacement.
4. The location/buffer-identity spec promoted in M3h's cleanup
   (M3h's note: "equivalent end-to-end coverage moves to M3h's
   `spec/unit/materialize_spec.rb` + the canonical-dump compat
   tests"; if location coverage ended up living as its own file,
   that file is also out of scope here).

(1) and (2) are the targets. `RBS::Writer#write_decl`
(`vendor/rbs/lib/rbs/writer.rb:119-204`) covers every variant
materialization emits and exercises the printed-shape-bearing fields
(name, type_params, super_class / self_types, members, type, comment,
annotations). For per-decl checks Writer is strictly stronger than
`is_a?` chains and strictly cheaper than maintaining JSON
normalizers.

## Scope

### `spec/support/writer_oracle.rb` (new)

A small support module exposing two helpers:

```ruby
module Librbs::SpecSupport
  module WriterOracle
    # Print one decl through `RBS::Writer` (non-preserve mode) and
    # return the resulting String. Accepts a single decl or an Array
    # of decls (used for open classes via `ClassEntry#each_decl`).
    def self.write(decl_or_decls)
      decls = Array(decl_or_decls)
      io = StringIO.new
      RBS::Writer.new(out: io).write(decls)
      io.string
    end

    # Run the same env-build script in a fresh ruby subprocess
    # without librbs loaded, locate the named decl, and return its
    # `WriterOracle.write` output. Used as the pure-RBS oracle.
    def self.write_pure(env_script, type_name)
      # delegates to spec/support/without_librbs.rb, returns String
    end
  end
end
```

`write` is the only Writer entry point used in M3j specs. Tests must
not call `RBS::Writer.new` directly so the non-preserve invariant
(see "Design constraints") is centralized.

### `spec/unit/materialize_spec.rb` (rewrite the entry-shape block)

The M3h spec asserts shape via `is_a?` chains:

```ruby
expect(entry.each_decl.first).to be_a(RBS::AST::Declarations::Class)
# ...member-level is_a? assertions...
```

Replace those examples with Writer golden-string examples covering
each AST variant materialization emits — the eight `write_decl`
branches plus open-class plus one nested decl. Each example:

1. builds a small inline env via `with_inline_env` containing exactly
   the shape under test,
2. picks the relevant entry out of the matching `*_decls` Hash,
3. extracts the decl(s) (`entry.decl` for `SingleEntry` subclasses;
   `entry.each_decl.to_a` for `Class`/`ModuleEntry`),
4. asserts `WriterOracle.write(decls)` equals a heredoc-fixed golden
   string.

The Writer's `case decl` covers Class / Module / Interface /
TypeAlias / Constant / Global / ClassAlias / ModuleAlias dispatch
implicitly — if materialization produces the wrong AST class, the
golden-string compare fails just as cleanly as `is_a?`, and it
additionally pins down every printed field (name, type_params,
super_class, self_types, members up to one level, type, comment,
annotations).

Keep the **re-entrancy** and **pure-Ruby env fallback** examples
unchanged. Those test concerns Writer doesn't cover (object identity
on repeat accessor calls; the `instance_variable_defined?` guard in
`ensure_materialized`).

### `spec/compat/object_spec.rb` → `spec/compat/per_decl_writer_spec.rb` (rename + rewrite)

Same per-name parity shape, Writer-based oracle:

- Same fixed core set as M3h's `object_spec.rb` (`::Object`,
  `::Integer`, `::String`, `::Array`, `::Hash`, `::Numeric`, plus
  whatever M3h actually shipped — keep parity with M3h, don't
  expand here).
- For each name, both the librbs-side and the pure-RBS-side build
  the default core env, look up `class_decls[name]`, run
  `entry.each_decl.to_a` through `WriterOracle.write`, and compare
  the resulting strings.
- Repeat with `resolve_type_names` applied. The resolved variant is
  where Writer-based comparison earns its keep — `to_json` shows
  raw absolutized names; Writer renders them in the user-visible
  syntax, which is what we actually care about staying compatible
  with.

Delete `object_spec.rb` after the new spec is green and observed to
fail under seeded mutations.

### Specs to keep as-is

- `spec/support/canonical_dump.rb` and the M3i-finalized
  `core_spec.rb` / `core_stdlib_spec.rb` / `gems_spec.rb`. These
  exercise **bulk parity** across populated environments; per-decl
  Writer iteration would re-walk the same data thousands of times
  for no marginal coverage. canonical_dump remains the "did the six
  tables fill correctly" oracle.
- Whatever spec owns location / buffer-identity assertions after
  M3h's cleanup. Locations are not in Writer's printed output
  (non-preserve mode), so this slice is not a candidate replacement.
- `spec/unit/materialize_spec.rb`'s re-entrancy and pure-Ruby
  fallback examples (only the entry-shape examples are rewritten).

## Design constraints

- **Non-preserve mode only.** `RBS::Writer#write_loc_source`
  (`writer.rb:306-313`) reads `loc.source` from the original parser
  buffer when `preserve?` is true. Materialization-produced AST
  carries synthetic locations whose buffer is the `with_inline_env`
  temp file, not the source the test reasons about; in `preserve!`
  mode the printed output drifts from the golden string by
  surrounding whitespace and comments. The `WriterOracle` helper
  centralizes the non-preserve choice.
- **Per-decl, not per-env.** `RBS::Writer#write` accepts the same
  set of inputs `Environment#declarations` returns, but feeding it a
  whole environment re-introduces the source-order non-determinism
  that canonical_dump deliberately sorts away. M3j tests always
  pass a single decl (or a single entry's `each_decl.to_a`), never
  a whole env's declarations.
- **Open-class iteration order matches both sides.** Both the
  librbs-side and the pure-RBS-side must walk `each_decl` in the
  same order. Upstream `ClassEntry#each_decl` iterates
  `@context_decls` in insertion order; M3h's materialization
  preserves that order via the documented pre-order walk. Tests
  rely on this — if a regression breaks insertion order on the
  librbs side, the per-decl Writer parity spec is one of the
  things that should catch it.
- **No new patches, no native changes.** M3j edits only `spec/`,
  `spec/support/`, and (if necessary) the slice indices in
  `docs/tasks/milestones/M3/`. If a Writer-based spec exposes a
  real materialization bug, file it as a fix on M3h (or the
  relevant earlier slice) rather than working around it in test
  code.
- **Don't expand the curated set in this slice.** The temptation
  is to grow the per-name set now that the oracle is cheaper.
  Resist; broadening coverage is what M3i's bulk matrix already
  does. Same-shape, stronger-oracle is the M3j contract.

## Out of scope (deferred)

- A canonical-dump re-implementation in Rust, or any change to the
  bulk-parity matrix from M3i. Tracked under the existing
  canonical_dump followup.
- Round-tripping a full env through Writer back into a parser as a
  consistency check. Interesting but separate; would need a parser
  pass too. File a followup if M4 wants it.
- Writer-based golden-file coverage for `RBS::AST::Ruby::*` (inline
  ruby annotations). Materialization doesn't emit these today; if
  M5 adds Ruby-source ingestion, extend then.
- Re-introducing per-shape unit specs that M3h removed. M3j does
  not bring back deleted spec files; if a Writer-based example
  needs more granular coverage than the rewritten
  `materialize_spec.rb` provides, add it as another example in
  that file rather than as a new file.

## Acceptance

- [x] `spec/support/writer_oracle.rb` exists and is the only place
      `RBS::Writer.new` is constructed in the spec tree.
- [x] `spec/unit/materialize_spec.rb`'s entry-shape block is
      Writer-driven, covering the eight `write_decl` branches plus
      open-class plus one nested-decl case. Re-entrancy and
      pure-Ruby fallback examples are unchanged.
- [x] `spec/compat/per_decl_writer_spec.rb` is green for the same
      curated core set M3h's `object_spec.rb` covered, both
      unresolved and `resolve_type_names`-applied. The Writer
      oracle initially exposed a real M3h bug (the entry's
      absolute key was being reused as `decl.name`); the fix lands
      in this slice as a one-spot native change in
      `ext/librbs/src/materialize/decl.rs` — each `materialize_*_node`
      derives its own decl-self name from the literal AST `name_node`
      via the new `literal_decl_name` helper, so the source form
      (relative vs. absolute) is preserved.
- [x] `spec/compat/object_spec.rb` is deleted, not commented out.
      (The M3h cleanup stopped short of materialising it as its own
      file; the per-name parity coverage M3h had landed inline in
      `materialize_spec.rb` is now superseded by
      `per_decl_writer_spec.rb`.)
- [x] `canonical_dump` bulk-parity specs from M3i remain unchanged
      and green.
- [x] The location/buffer-identity spec (whatever M3h's cleanup
      produced) remains unchanged and green.
- [x] CI green; the diff touches `spec/`, `spec/support/`,
      `docs/tasks/`, and (for the M3h decl-self-name fix surfaced
      by this slice) `ext/librbs/src/materialize/decl.rs`.

## References

- `vendor/rbs/lib/rbs/writer.rb` (`write_decl` at L119, `write_loc_source`
  at L306, `preserve!` at L18)
- `vendor/rbs/lib/rbs/environment.rb` (entry shapes at L18-L46,
  `declarations` at L14)
- `vendor/rbs/lib/rbs/environment/class_entry.rb` (`each_decl` at L21)
- `spec/support/canonical_dump.rb` (the oracle this slice does not
  replace)
- `spec/support/without_librbs.rb` (subprocess driver reused for the
  pure-RBS oracle)
- M3h's "Cleanup" and "Tests" sections (the inputs M3j operates on)
