# M4 decision

Date: 2026-05-11

## The numbers (recap)

| script                | small | medium | large |
|-----------------------|-------|--------|-------|
| `load_only.rb`        | 1.03x | 1.22x  | 1.53x |
| `load_and_resolve.rb` | 1.83x | 2.00x  | 4.26x |

Resolve cost in librbs is essentially zero on every size; pure RBS
spends ~100ms (small) to ~1.2s (large) inside `resolve_type_names`. The
M3d resolver port is unambiguously paying off.

The load-only path now sits at 1.03x / 1.22x / 1.53x after the
post-baseline materialiser tuning (`TypeName` / `Namespace` flyweighting,
the `RBS::Location` children FFI fast path, the per-ctx static-Symbol
cache, and the `RBS::Types::Bases::*` `obj_alloc` + `ivar_set` fast
path — all documented in `M4-baseline.md`). `large` and `medium` are
clear wins for librbs; `small` has crept ahead of pure RBS but its
remaining margin is dominated by fixed parser+loader cost rather than
the materialiser.

## Mapping to the M4 decision flow

The flow in `docs/tasks/milestones/M4-decision-point.md` (Task §4) reads:

- `load_and_resolve >= 2x AND load_only >= 2x` → M4b. **Not matched** —
  `large` is closest (4.26x and 1.53x) but `load_only` does not clear
  2x.
- `load_and_resolve >= 3x AND load_only < 1.5x` → M4a. **Borderline on
  large** (4.26x and 1.53x): `load_only` now just clears 1.5x after the
  Bases / Symbol cache work, so the materialize ceiling is narrower
  than at the original M4 baseline.
- `load_and_resolve < 1.5x` → re-investigate M3. **Not matched** —
  `medium` reads 2.00x.

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
   materialization is a modest remaining ceiling on small/medium"; both
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

## What the next milestone should keep in mind

- The Core+Wrapper kickoff should pre-commit to lazy materialization of
  the six decl hashes and `sources` Array (see followups.md §"Source
  materialization granularity"). That gives the M4a benefit "for free"
  on the new architecture.
- The `small` load_only number (1.03x) is within noise of pure RBS. The
  parser+loader fixed cost dominates at that size, so further wins
  there will likely come from Core+Wrapper's lazier materialisation
  rather than from more local materialiser tuning.
- If the Core+Wrapper rebuild slips, revisit this decision and implement
  M4a as a stopgap. The materialize.rs surface (~2.5kLoC across nine
  files) is large but the design in M4 §5 is unchanged.
