# M4 decision

Date: 2026-05-17

## The numbers (recap)

Speedups against pure RBS across Ruby 3.3.11 / 3.4.9 / 4.0.4. `normal`
is the upstream-initializer path, `fast` is the `obj_alloc + ivar_set`
bypass (default). Corpus: core (small) and kaigionrails/conference-app's
92-gem RBS collection (large) — see `M4-baseline.md` for the full
tables, raw timings, and environment.

| ruby   | script                | small (normal / fast) | large (normal / fast)  |
|--------|-----------------------|-----------------------|------------------------|
| 3.3.11 | `load_only.rb`        | 1.18x / **1.78x**     | 2.27x / **4.84x**      |
| 3.3.11 | `load_and_resolve.rb` | 2.11x / **3.39x**     | 4.90x / **9.63x**      |
| 3.4.9  | `load_only.rb`        | 1.13x / **1.50x**     | **0.76x** / **1.29x**  |
| 3.4.9  | `load_and_resolve.rb` | 1.54x / **2.49x**     | 1.50x / **2.41x**      |
| 4.0.4  | `load_only.rb`        | **0.97x** / **1.49x** | **0.70x** / **1.33x**  |
| 4.0.4  | `load_and_resolve.rb` | 1.56x / **2.24x**     | 1.50x / **2.27x**      |

(Cells where librbs trails pure RBS are bolded for visibility; bold in
the fast column flags the headline number for that row.)

Ruby 3.4 makes Prism the default parser and pulls a large chunk of the
parser cost into pure RBS itself. The librbs **normal** path now
trails pure RBS on 3.4+ `large` `load_only` (0.76x / 0.70x) — the
materializer's `:initialize` funcall overhead exceeds what librbs
saves on the parser side once pure RBS has Prism. The **fast alloc**
bypass (PR #51, `obj_alloc + ivar_set` for every materializer call site
whose upstream `initialize` is a pure ivar sequence; stacking on prior
`TypeName` / `Namespace` flyweighting, the `RBS::Location` FFI fast
paths, the per-ctx Symbol cache, and the `rbs_new_location2` bridge —
see `M4-baseline.md`) is the only librbs configuration that still wins
on every cell.

## Mapping to the M4 decision flow

The flow in `docs/tasks/milestones/M4-decision-point.md` (Task §4) reads:

- `load_and_resolve >= 2x AND load_only >= 2x` → M4b. **Matched only
  on Ruby 3.3.11 / `large`** (4.90x / 2.27x normal; 9.63x / 4.84x
  fast). On 3.4+ `load_only` is below 2x in every cell, so M4b's
  joint threshold no longer triggers regardless of Ruby version.
- `load_and_resolve >= 3x AND load_only < 1.5x` → M4a. **Not matched**
  anywhere — `load_and_resolve` only clears 3x in fast-alloc mode on
  3.3.11 (3.39x small, 9.63x large), and in those cells `load_only` is
  not below 1.5x.
- `load_and_resolve < 1.5x` → re-investigate M3. **Not matched** —
  the smallest `load_and_resolve` cell is 1.50x (3.4.9 large normal /
  4.0.4 large normal), right at the threshold but above it.

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
   overhead costs. The benchmark above answers it: the resolve win is
   real but compresses on 3.4+ as pure RBS adopts Prism, and the
   materializer is now the dominant cost — normal-mode librbs even
   trails pure RBS on 3.4+ `large` `load_only`. Both insights argue
   for moving directly to the Core+Wrapper kickoff (which restructures
   materialisation) rather than spending M4a effort on a partial-patch
   plumbing layer whose Ruby-version safety margin is already eroding.

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
- If the Core+Wrapper rebuild slips, revisit this decision and implement
  M4a as a stopgap. The materialize.rs surface (~2.5kLoC across nine
  files) is large but the design in M4 §5 is unchanged.
