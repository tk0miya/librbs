# M4 decision

Date: 2026-05-11

## The numbers (recap)

Speedups against pure RBS. `normal` is the upstream-initializer path,
`fast` is the `obj_alloc + ivar_set` bypass (default).

| script                | small (normal / fast)  | large (normal / fast) |
|-----------------------|------------------------|-----------------------|
| `load_only.rb`        | 1.06x / 0.95x          | 1.48x / **2.11x**     |
| `load_and_resolve.rb` | 1.79x / 1.83x          | 4.04x / **4.87x**     |

Resolve cost in librbs is essentially zero on either side; pure RBS
spends ~120ms (small) to ~2.1s (large) inside `resolve_type_names`.
The M3d resolver port is unambiguously paying off.

The load-only path on `large` clears 2x in fast-alloc mode after the
materialiser tuning stack (`TypeName` / `Namespace` flyweighting, the
`RBS::Location` children FFI fast path, the per-ctx static-Symbol
cache, the `RBS::Types::Bases::*` `obj_alloc` + `ivar_set` fast path,
and the `rbs_new_location2` FFI fast path for `RBS::Location.new`
itself — all documented in `M4-baseline.md`). `large` is a clear win
in both modes; `small` is within run-to-run noise of pure RBS, with
the remaining margin dominated by fixed parser+loader cost rather
than the materialiser.

## Mapping to the M4 decision flow

The flow in `docs/tasks/milestones/M4-decision-point.md` (Task §4) reads:

- `load_and_resolve >= 2x AND load_only >= 2x` → M4b. **Matched on
  `large` in fast-alloc mode** (4.87x and 2.11x); not matched in
  normal mode (4.04x and 1.48x).
- `load_and_resolve >= 3x AND load_only < 1.5x` → M4a. **No longer
  matched in fast-alloc mode on large** (load_only clears 1.5x at
  2.11x); still applicable in normal mode (1.48x is just below the
  threshold).
- `load_and_resolve < 1.5x` → re-investigate M3. **Not matched** —
  smallest cell is 1.79x.

## Decision: defer implementation; record baseline only

Per discussion with the maintainer, M4 closes as a measurement-and-
record milestone. Neither M4a nor M4b is being implemented in this
milestone. Reasoning:

1. **M4a as sketched would be largely throwaway work**. The accepted long-
   term direction in `docs/tasks/followups.md#open` (first item,
   "Reimplement `RBS::Environment` and `RBS::EnvironmentLoader` as
   Core+Wrapper") subsumes the per-Entry handle / lazy-materialize
   plumbing M4a would add. That followup is the agreed shape post-M4 and
   was deliberately deferred until M4 told us how much partial-patch
   overhead costs. The benchmark above answers "the resolve win is real,
   materialization is a modest remaining ceiling on small"; both
   insights flow directly into the Core+Wrapper kickoff.

2. **M4b ("compatibility-API completion") also gets folded into the
   Core+Wrapper rebuild**. Its motivation is closing the silent-
   inconsistency surface in `lib/librbs/patches/environment.rb` (methods
   that fall through to upstream and read empty ivars). The Core+Wrapper
   architecture eliminates that whole class of bug structurally rather
   than method-by-method, so adding a few `ensure_materialized` hops now
   would not retire the followup — the followup still has to land.

3. **The benchmark is now reproducible**. The harness fixes captured in
   `M4-baseline.md` (Bundler env leakage in the subprocess, rubygems-
   sourced library lookup in `build_environment`) mean re-running the
   pipeline before/after the Core+Wrapper rebuild produces directly
   comparable numbers. That is the actual value M4 was asked to deliver.

## Closing acceptance: upstream env tests in CI

The "manual Steep smoke test" acceptance item is replaced by a stronger,
reproducible check: the three upstream tests that exercise the
`RBS::Environment` / `RBS::EnvironmentLoader` / `RBS::EnvironmentWalker`
surface — `environment_test.rb`, `environment_loader_test.rb`,
`environment_walker_test.rb` — were copied verbatim from
`vendor/rbs/test/rbs/` into `test/rbs/` and now run under
`require "librbs"` on every CI Ruby (`upstream-env-test` job). 50/50
tests pass (2 omissions for the unavailable `rbs-amber` gem, identical
to upstream). Closing this job green is the operational definition of
"M4 is done"; the manual Steep verification was retained as a safety
net but is no longer load-bearing.

Patch change that this surfaced: `resolve_type_names` now falls through
to upstream when `@__librbs_handle` is absent. Upstream's env tests
routinely call `Environment.new.add_source(...).resolve_type_names`,
which has no Rust state to resolve against. The pre-fix patch raised
`RBS::Environment has no @__librbs_handle`. The fix matches the
existing fallback in `class_decls` and friends (the
`instance_variable_defined?(:@__librbs_handle)` guard), so the
pure-Ruby env path is uniformly preserved across every patched method.

## What the next milestone should keep in mind

- The Core+Wrapper kickoff should pre-commit to lazy materialization of
  the six decl hashes and `sources` Array (see followups.md §"Source
  materialization granularity"). That gives the M4a benefit "for free"
  on the new architecture.
- The `small` load_only numbers (1.06x normal / 0.95x fast) sit within
  noise of pure RBS. The parser+loader fixed cost dominates at that
  size, so further wins there will likely come from Core+Wrapper's
  lazier materialisation rather than from more local materialiser
  tuning.
- If the Core+Wrapper rebuild slips, revisit this decision and implement
  M4a as a stopgap. The materialize.rs surface (~2.5kLoC across nine
  files) is large but the design in M4 §5 is unchanged.
