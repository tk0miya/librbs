# M4: Benchmark Measurement and Next-Phase Decision

## Goal

Measure performance at the M3 completion point and **decide whether to
proceed with per-Entry lazy materialization (M4a)**.

This milestone is primarily about **measurement, decision, and recording**
rather than implementation. The decision branches into M4a or M4b.

## Prerequisites

- M3 is complete.
- `from_loader → resolve_type_names → class_decls.size` works end-to-end on
  both pure RBS and librbs.

## Tasks

### 1. Set up the benchmark suite ✅

Two cold-start scripts live under `benchmark/`, each driving the workload
across all three sizes and both implementations:

```
benchmark/
├── load_only.rb          # from_loader + materialize
├── load_and_resolve.rb   # from_loader + resolve_type_names + materialize
└── helpers.rb
```

Both scripts call `class_decls.size` at the end so the librbs path
finishes its one-shot `materialize_all` and we are comparing fully
realized Ruby state on both sides — pure-Rust work the Ruby caller
never observes is excluded from the comparison. The difference between
the two scripts is therefore purely the cost of `resolve_type_names`.

A dedicated `full_use.rb` (load + resolve + materialize) and a
`steep_simulation.rb` (per-decl walk) were considered and dropped:
materialization is already in-band on both scripts, and the per-decl
walk does not add new Ruby-visible work after `materialize_all` has
populated every Entry. If a future profile suggests post-materialize
iteration is itself a hotspot, reintroduce a script then.

Pure RBS and librbs cannot coexist in one process (`require "librbs"`
patches `RBS::Environment` globally), so each measurement runs in its
own subprocess via `BenchHelpers.run_subprocess`.

### 2. Measurement matrix

Three input sizes:

| Case | Contents |
|---|---|
| **small** | core only |
| **medium** | core + a representative subset of stdlib (pathname, date, time, uri, ...) |
| **large** | core + the gem RBS collection vendored from SeleniumHQ/selenium's `rbs_collection.lock.yaml` (~33 gems via gem_rbs_collection) |

That's 3 sizes × 2 benchmark scripts = 6 numbers.

The `large` size requires a one-shot `rbs collection install` step —
see `benchmark/README.md` for the exact command.

### 3. Record results

Write results to `benchmark/results/M4-baseline.md`:

```markdown
# M4 baseline benchmark

Date: YYYY-MM-DD
Environment: macOS 14.x / Ruby 3.4.x / Apple M2 (or Linux x86_64 / ...)

## load_only.rb

| size | pure RBS | librbs (M3) | speedup |
|---|---|---|---|
| small | XXX ms | XXX ms | X.Xx |
| medium | XXX ms | XXX ms | X.Xx |
| large | XXX ms | XXX ms | X.Xx |

## load_and_resolve.rb

...
```

### 4. Decision

Use this flow to choose the next step. `load_and_resolve - load_only`
is the wall-time cost of `resolve_type_names` alone; comparing the two
speedups tells us where the win is concentrated.

```
- load_and_resolve >= 2x AND load_only >= 2x:
    → Both phases are paying off. M4a (per-Entry lazy
       materialization) is unnecessary. Proceed to M4b
       (compatibility-API completion). Goal achieved.

- load_and_resolve >= 3x AND load_only < 1.5x:
    → Resolve is fast but `from_loader + materialize` dominates
       wall time. Materialization is the suspected bottleneck;
       M4a is worth doing.

- load_and_resolve < 1.5x:
    → Something is wrong with M3 (the Rust port of the resolver isn't
       paying off). Re-investigate M3 with a profiler.
```

Record the decision in `benchmark/results/M4-decision.md`:

- The numbers
- Which case is how many times faster
- Whether to proceed with M4a or M4b
- The reasoning

### 5. M4a path: additional implementation

Per-Entry lazy materialization:

- Patch `RBS::Environment::ClassEntry` etc. so that `each_decl` /
  `context_decls` / `primary_decl` materialize their decls on first call.
- Replace `materialize_all` with a coarser `materialize_class_decls_keys`
  Native API that creates only the keys and Entry shells.
- Give Entries a Rust handle ivar and lazy-materialize per method.

Detailed design happens at decision time. This document only sketches the
shape.

### 6. M4b path: compatibility-API completion

Cover other `RBS::Environment` methods that Steep / the investigation
revealed:

- `each_type_name`
- `validate_type_params`
- `each_rbs_source` / `each_ruby_source`
- `inspect`
- `buffers`
- ...

These should work via the librbs path. Most should be handled by routing
through `ensure_materialized`, requiring minimal additional code.

## Acceptance

- [ ] `benchmark/results/M4-baseline.md` records all 6 numbers.
- [ ] `benchmark/results/M4-decision.md` records the decision and reasoning.
- [ ] Either M4a or M4b is implemented.
- [ ] Manually verify that running Steep on a real project produces the
      same results as before the change.

## Pitfalls and mitigation

### Cold start vs steady state

Loader code is paid once per process; cold start matters more than
steady-state throughput. The two scripts therefore measure
`Benchmark.realtime` of a freshly built loader on each repeat (see
`BenchHelpers.measure_realtime`) and report the minimum of N runs.

`BenchHelpers.measure_ips` is also provided for steady-state ips
comparison via `benchmark-ips`; reach for it only if a specific
hypothesis needs it.

### Subprocess isolation

`require "librbs"` patches `RBS::Environment` globally and there is no
clean way to undo that in the same process. `helpers.rb` runs each
(impl × size) combination in its own `ruby -e` subprocess via
`run_subprocess`, with Bundler env stripped so collection-pulled gems
resolve.

### Selecting workloads for the large case

The `large` size sources its gem list from a real-world OSS project's
`rbs_collection.lock.yaml` (currently SeleniumHQ/selenium, ~33 gems
pinned to a specific gem_rbs_collection revision). Swap in a different
project's lockfile rather than hand-curating gem lists — picking gems
ad-hoc tends to drift toward whatever the author uses, while a
real lockfile reflects what an actual Steep adopter ships.

## Next milestone

Depending on the outcome:
- If M4a is required, design and implement it, then move on to M5.
- Otherwise, go directly to M5.

→ [M5-incremental.md](M5-incremental.md)
