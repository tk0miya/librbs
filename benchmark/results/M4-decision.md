# M4 decision

Date: 2026-05-10

## The numbers (recap)

| script                | small | medium | large |
|-----------------------|-------|--------|-------|
| `load_only.rb`        | 0.76x | 0.78x  | 1.04x |
| `load_and_resolve.rb` | 1.60x | 1.33x  | 3.13x |

Resolve cost in librbs is essentially zero on every size; pure RBS spends
~190ms (small/medium) to ~2.2s (large) inside `resolve_type_names`. The
M3d resolver port is unambiguously paying off.

The same is **not** true of the load-only path: librbs is slightly slower
than pure RBS on small/medium and ties on large. The `materialize_all` walk
(building Ruby `*Decl` / `*Member` objects from Rust state) costs
approximately the same as pure RBS's parser+indexer. None of the work
done before `add_source` is visible to the benchmark caller, so the
materialize step is the only Ruby-visible payload, and its cost is
roughly equal to upstream's full from_loader.

## Mapping to the M4 decision flow

The flow in `docs/tasks/milestones/M4-decision-point.md` (Task §4) reads:

- `load_and_resolve >= 2x AND load_only >= 2x` → M4b. **Not matched** — no
  size hits both thresholds simultaneously.
- `load_and_resolve >= 3x AND load_only < 1.5x` → M4a. **Matched on
  large** (3.13x and 1.04x).
- `load_and_resolve < 1.5x` → re-investigate M3. Medium technically falls
  here (1.33x), but the breakdown above shows the resolver port is fine;
  the gap is on the load side, not the resolve side.

The headline signal — large workload, which is closest to a real Steep
run — points at **M4a (per-Entry lazy materialization)**: resolve is free,
load+materialize dominates wall time, materialization is the suspected
bottleneck.

## Decision: defer implementation; record baseline only

Per discussion with the maintainer, M4 is closing as a measurement-and-
record milestone. Neither M4a nor M4b is being implemented in this commit.
Reasoning:

1. **M4a as sketched would be largely throwaway work**. The accepted long-
   term direction in `docs/tasks/followups.md#open` (first item,
   "Reimplement `RBS::Environment` and `RBS::EnvironmentLoader` as
   Core+Wrapper") subsumes the per-Entry handle / lazy-materialize
   plumbing M4a would add. That followup is the agreed shape post-M4 and
   was deliberately deferred until M4 told us how much partial-patch
   overhead costs. The benchmark above answers "the resolve win is real
   but materialization is a ceiling on small/medium"; both insights flow
   directly into the Core+Wrapper kickoff.

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
- The medium-size load_and_resolve number (1.33x) is the soft spot to
  watch. If, after Core+Wrapper, medium still does not clear 2x, profile
  the load-only path — the resolve phase is already saturated.
- If the Core+Wrapper rebuild slips, revisit this decision and implement
  M4a as a stopgap. The materialize.rs surface (~2.5kLoC across nine
  files) is large but the design in M4 §5 is unchanged.
