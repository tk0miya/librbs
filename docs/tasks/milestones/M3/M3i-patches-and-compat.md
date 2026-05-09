# M3i: Patches polish + full compat matrix

## Goal

Close every remaining acceptance item on the parent M3 doc:

- canonical dumps for **core + stdlib** match pure RBS exactly,
- the **major-gems matrix** is green,
- all CI jobs are green.

By the time this slice starts, the native bridge and materialization are
already working; this slice broadens test coverage, irons out edge-case
divergences, and finishes the patch layer.

## Prerequisites

- M3a + M3b + M3c + M3d + M3e + M3f + M3g + M3h merged.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  sections "Compatibility tests" and "Test matrix".

## Scope

### `spec/compat/`

Add three spec files, each running both the librbs-patched path and a
fresh-subprocess pure-RBS path, and asserting `canonical_dump` equality:

- `core_spec.rb` — already partly covered by M3c/M3d; canonicalize and
  retain.
- `core_stdlib_spec.rb` — `RBS::EnvironmentLoader.new` then
  `loader.add(library: "...")` for every stdlib library shipped with the
  vendored `vendor/rbs/stdlib` tree.
- `gems_spec.rb` — parametrized over `json`, `set`, `bigdecimal`, `csv`,
  `pathname`, `tempfile`, `time`, `uri`. Skip a gem if its sigs aren't
  installed and emit a `pending`.

`spec/support/subprocess.rb` runs the pure-RBS dump in a fresh ruby
subprocess with `LIBRBS_DISABLE=1` (or simply not requiring `librbs`). The
subprocess prints the canonical dump to stdout; the parent compares.

### `spec/support/canonical_dump.rb`

Audit and finalize the Ruby-side dumper — every divergence found while
running the matrix becomes a fix in this file (or in the Rust dumper if
the followup "Rust-side `canonical_dump` implementation" has been
applied, or in both, but always so that they stay in lockstep with the
canonical-dump format spec authored in M3c).

### Patch hardening

- `lib/librbs/patches/environment.rb`: confirm the
  `instance_variable_defined?(:@__librbs_handle)` fallback survives the
  full matrix. Add a regression spec where a user instantiates
  `RBS::Environment.new` directly and accesses `class_decls` — must not
  raise, must not call into native.
- `lib/librbs/patches/environment_loader.rb`: revisit; if no overrides
  are needed, delete the placeholder file (don't leave dead code).

### CI

- Add a `compat-test` job to the existing GitHub Actions workflow that
  runs `bundle exec rspec spec/compat`.
- The matrix runs on the same Ruby versions already covered by the unit
  job. No Windows. (Per project README "non-goals".)
- Cache `target/` between runs to keep the job under ~5 minutes.

### Native-purity audit (final)

Re-run the audit from M3d on the resolver path now that materialization
exists — confirm that `from_loader` and `resolve_type_names` still don't
touch Ruby beyond the documented ivar reads. Materialization (only
triggered by the `*_decls` accessors) is the single permitted exception.

### Followups housekeeping

- For every divergence found that can't be fixed cleanly within this
  slice, file a followup in `docs/tasks/followups.md` with the trigger
  ("M4 benchmark" or "before M5 incremental").
- M2 followup "DeclRef indexing consistency" should be **closed** by now
  via M3b's round-trip test — verify and remove from `followups.md`.

## Out of scope (deferred)

- Per-Entry lazy materialization — M4 decision point.
- Benchmarks — M4.
- Incremental updates — M5.
- `add_source` patch — M5 (only if the M5 design needs it).

## Acceptance

- [x] `spec/compat/core_spec.rb`, `core_stdlib_spec.rb`, `gems_spec.rb`
      all green.
- [x] CI's `compat-test` job runs on every PR.
- [x] Direct `RBS::Environment.new` (pure path, no librbs handle) works
      end-to-end through the patched accessors.
- [x] Native-purity audit re-confirmed.
- [x] Parent M3 doc's acceptance section fully checked off.
- [x] Closed M2 followups removed from `followups.md`.

## References

- `vendor/rbs/lib/rbs/environment.rb` (driver semantics for `only:`)
- `vendor/rbs/stdlib/` (the matrix's input set)
