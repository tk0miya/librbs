# M4 decision

Date: 2026-05-10

## The numbers (recap)

| script                | small | medium | large |
|-----------------------|-------|--------|-------|
| `load_only.rb`        | 0.81x | 0.72x  | 0.97x |
| `load_and_resolve.rb` | 1.41x | 1.69x  | 2.81x |

Resolve cost in librbs is essentially zero on every size; pure RBS spends
~110ms (small) to ~1.6s (large) inside `resolve_type_names`. The M3d
resolver port is unambiguously paying off.

The same is **not** true of the load-only path: librbs is slightly slower
than pure RBS on small/medium and roughly ties on large. The
`materialize_all` walk (building Ruby `*Decl` / `*Member` objects from
Rust state) costs approximately the same as pure RBS's parser+indexer.
None of the work done before `add_source` is visible to the benchmark
caller, so the materialize step is the only Ruby-visible payload, and
its cost is roughly equal to upstream's full from_loader.

Two materialize-side optimisations on this branch — pre-sizing every
`RArray` / `RHash` / `Vec` via `ary_new_capa(n)` / `hash_new_capa(n)`
(`88bbb88`) and allocating `RBS::Types::Bases::*` via
`RClass#obj_alloc` + `ivar_set` (`c3670a4`) — together trimmed the
materialize-only median by ≈15% (small), ≈19% (medium), and ≈11%
(large). That moved the load_and_resolve speedup at medium from 1.33x →
1.69x and at large from 3.13x → 2.81x (pure RBS large also got faster
on the second run, which is why the large speedup ratio dipped despite
the absolute librbs time dropping ≈19%). The load-only path is still
sub-1x on small/medium because cold-start time is dominated by
parser/loader plumbing outside materialize.

## Mapping to the M4 decision flow

The flow in `docs/tasks/milestones/M4-decision-point.md` (Task §4) reads:

- `load_and_resolve >= 2x AND load_only >= 2x` → M4b. **Not matched** — no
  size hits both thresholds simultaneously.
- `load_and_resolve >= 3x AND load_only < 1.5x` → M4a. **Not matched** —
  large now sits at 2.81x (post-tuning) rather than the 3.13x recorded
  at the original M4 read-off.
- `load_and_resolve < 1.5x` → re-investigate M3. Small (1.41x) now falls
  here; medium (1.69x) and large (2.81x) do not. The breakdown above
  still shows the resolver port is fine, so the small-size dip is a
  cold-start ceiling rather than a resolver issue.

No single threshold cleanly fires after the materialize tuning. The
headline signal — large workload, which is closest to a real Steep
run — still points at **M4a (per-Entry lazy materialization)** as the
work most likely to lift load_only past 1x on small/medium: resolve is
free, load+materialize dominates wall time, and the remaining
materialize cost is now concentrated in `make_location` and the
keyword-arg path on `Types::*` / `Members::*` (see "Pointers" below).

## Decision: defer implementation; record current state

Per discussion with the maintainer, M4 is closing as a measurement-and-
record milestone. Neither M4a nor M4b is being implemented in this
branch beyond the two narrowly-scoped materialize tweaks already
committed. Reasoning:

1. **M4a as sketched would be largely throwaway work**. The accepted
   long-term direction in `docs/tasks/followups.md#open` (first item,
   "Reimplement `RBS::Environment` and `RBS::EnvironmentLoader` as
   Core+Wrapper") subsumes the per-Entry handle / lazy-materialize
   plumbing M4a would add. That followup is the agreed shape post-M4
   and was deliberately deferred until M4 told us how much partial-patch
   overhead costs. The benchmark above answers "the resolve win is real
   but materialization is a ceiling on small/medium even after
   per-allocation tuning"; both insights flow directly into the
   Core+Wrapper kickoff.

2. **M4b ("compatibility-API completion") also gets folded into the
   Core+Wrapper rebuild**. Its motivation is closing the silent-
   inconsistency surface in `lib/librbs/patches/environment.rb` (methods
   that fall through to upstream and read empty ivars). The Core+Wrapper
   architecture eliminates that whole class of bug structurally rather
   than method-by-method, so adding a few `ensure_materialized` hops now
   would not retire the followup — the followup still has to land.

3. **The benchmark is reproducible**. The harness fixes captured in
   `M4-baseline.md` (Bundler env leakage in the subprocess, rubygems-
   sourced library lookup in `build_environment`) plus the focused
   in-process materialize-only timing make re-running the pipeline
   before/after the Core+Wrapper rebuild produce directly comparable
   numbers. That is the actual value M4 was asked to deliver.

## Pointers for the next milestone

- The Core+Wrapper kickoff should pre-commit to lazy materialization of
  the six decl hashes and `sources` Array (see followups.md §"Source
  materialization granularity"). That gives the M4a benefit "for free"
  on the new architecture.
- After the per-allocation tuning on this branch, the remaining
  materialize cost is concentrated in two places worth tackling next:
    1. `make_location` — every node builds an `RBS::Location` plus zero
       or more sub-locations via `add_required_child` /
       `add_optional_child`, each of which is a Ruby method dispatch.
       Lifting the sub-location population into a single Ruby call (or
       extending the `obj_alloc + ivar_set` fast-path to the `Location`
       class) is the most direct next win given how many Locations get
       built (one per AST node plus 1–4 sub-locations).
    2. `kwargs!` packing for the larger `Types::*` and `Members::*`
       classes. The same `obj_alloc + ivar_set` trick used for
       `Bases::*` (`c3670a4`) applies wherever upstream's `initialize`
       is a vanilla `@a = a; @b = b; …`. Candidates by occurrence
       frequency: `Types::Variable`, `Types::ClassInstance`,
       `Types::Optional`, `Types::Union`, `Types::Tuple`, then the
       various `Members::*`. Each one needs the upstream `initialize`
       checked first — anything that freezes a collection or computes
       a derived ivar must keep the keyword-arg path or replicate the
       post-init step.
- A secondary follow-up: pre-cache `Id`s for the hot ivar names
  (`@location`, `@name`, `@type`, …). Each `ivar_set("@location", loc)`
  currently re-interns the string; a `Lazy<Id>` cached on
  `MaterializeCtx` (or alongside `ClassRefs`) would skip that.
- The small-size load_and_resolve number (1.41x) is the soft spot to
  watch. If, after Core+Wrapper, small still does not clear 2x, profile
  the load-only path — the resolve phase is already saturated.
- If the Core+Wrapper rebuild slips, revisit this decision and implement
  M4a as a stopgap. The materialize.rs surface (~2.5kLoC across nine
  files) is large but the design in M4 §5 is unchanged.
