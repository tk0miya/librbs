# M4 decision

Date: 2026-05-11

## The numbers (recap)

| script                | small | medium | large |
|-----------------------|-------|--------|-------|
| `load_only.rb`        | 0.83x | 0.95x  | 1.50x |
| `load_and_resolve.rb` | 1.57x | 1.52x  | 3.75x |

Resolve cost in librbs is essentially zero on every size; pure RBS
spends ~115ms (small) to ~1.6s (large) inside `resolve_type_names`. The
M3d resolver port is unambiguously paying off.

The load-only path sits at 0.83x / 0.95x / 1.50x. `large` is a clear
win for librbs; `medium` is essentially even with pure RBS; `small`
still tracks pure RBS because its fixed parser+loader cost dominates.

## Mapping to the M4 decision flow

The flow in `docs/tasks/milestones/M4-decision-point.md` (Task §4) reads:

- `load_and_resolve >= 2x AND load_only >= 2x` → M4b. **Not matched** —
  `large` is closest (3.75x and 1.50x) but `load_only` does not clear
  2x.
- `load_and_resolve >= 3x AND load_only < 1.5x` → M4a. **Not matched
  cleanly**: `load_only` sits at the 1.5x boundary on `large`, so the
  signal (materialize is a ceiling) is weaker than a clean M4a match.
- `load_and_resolve < 1.5x` → re-investigate M3. **Not matched** —
  `medium` reads 1.52x.

The headline workload (`large`) points at materialize still being the
remaining ceiling on the load-only side, but the ceiling is modest.

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
- The `medium` load_only number (0.95x) is the soft spot to watch. If,
  after Core+Wrapper, medium still does not clear pure RBS, profile the
  load-only path further — the resolve phase is already saturated.
- If the Core+Wrapper rebuild slips, revisit this decision and implement
  M4a as a stopgap. The materialize.rs surface (~2.5kLoC across nine
  files) is large but the design in M4 §5 is unchanged.
