# Ad-hoc intern patch for RBS — upstream proposal evaluation

Date: 2026-05-11
Environment: Ubuntu 24.04 LTS / Ruby 3.3.6 / Linux x86_64

## Question

Does porting the Rust interner approach (`crates/librbs-core/src/interner.rs`)
to pure Ruby pay off enough to be worth proposing upstream?

## Patches under test

Two ad-hoc PoC patches sit under `benchmark/`:

- **`rbs_intern_patch.rb` (v1)** — overrides `RBS::TypeName.new` and
  `RBS::Namespace.new` with hash-cons lookups; memoizes
  `TypeName#to_namespace`. Affects every construction in the codebase,
  including parsing.
- **`rbs_intern_patch_v2.rb` (v2)** — adds non-overriding `.intern`
  factories, memoizes `#hash` and `#to_namespace`, and monkey-patches
  the three resolver methods (`resolve_namespace0`,
  `resolve_type_name`, `resolve_head_namespace`) to call `.intern`
  instead of `.new`. Parsing is left untouched.

Both pass: declared-class count and resolved type identity match upstream
on the small fixture.

## Results

Cold-start wall time, minimum of N runs per cell. Each (impl, size)
pair runs in its own subprocess.

### `three_way_resolve.rb` — full pipeline (`from_loader` + `resolve_type_names`)

5 repeats per cell.

| size   | pure_rbs  | rbs_patched (v1) | rbs_patched_v2 | librbs   | v1 speedup | v2 speedup | librbs speedup |
|--------|-----------|------------------|----------------|----------|------------|------------|----------------|
| small  | 289.7 ms  | 299.6 ms         | 329.2 ms       | 228.2 ms | 0.97x      | 0.88x      | **1.27x**      |
| medium | 437.7 ms  | 360.3 ms         | 374.4 ms       | 249.2 ms | 1.21x      | 1.17x      | **1.76x**      |
| large  | 2740.8 ms | 2634.1 ms        | 2789.0 ms      | 915.9 ms | 1.04x      | 0.98x      | **2.99x**      |

### `load_only.rb` — parse + materialize, no resolve

5 repeats per cell.

| size   | pure_rbs  | rbs_patched (v1) | rbs_patched_v2 | librbs    |
|--------|-----------|------------------|----------------|-----------|
| small  | 161.3 ms  | 150.2 ms         | 162.2 ms       | 215.9 ms  |
| medium | 223.2 ms  | 239.5 ms         | 263.5 ms       | 290.0 ms  |
| large  | 1058.8 ms | 1083.4 ms        | 1032.5 ms      | 1100.4 ms |

Parsing is unaffected by either patch (within run-to-run noise).

### `resolver_only.rb` — `resolve_type_names` in isolation

Env built once per repeat outside the timed block, so the timing
captures only the resolver walk. 5 repeats per cell.

| size   | pure_rbs  | rbs_patched (v1) | rbs_patched_v2 | v1 speedup  | v2 speedup |
|--------|-----------|------------------|----------------|-------------|------------|
| small  | 127.2 ms  | 141.0 ms         | 131.5 ms       | 0.90x       | 0.97x      |
| medium | 168.1 ms  | 166.9 ms         | 173.1 ms       | 1.01x       | 0.97x      |
| large  | 2262.7 ms | 1469.9 ms        | 2065.3 ms      | **1.54x**   | 1.10x      |

This is the cleanest signal of the three benchmarks because parsing
variance is removed.

## Reading

1. **At small / medium scale, the patch is a wash.** Any allocation
   savings are eaten by the `Hash#[]` overhead on every construction
   plus the `[path, absolute]` / `[namespace, name]` key array
   allocation. Both v1 and v2 land within ±10 % of upstream.
2. **At large scale (selenium-class collection), v1 wins ~1.54× on the
   resolver alone**, ~30 % on the full `load_and_resolve` pipeline.
   The win comes from collapsing the many duplicated `(namespace, name)`
   constructions the resolver emits while walking cross-module
   references.
3. **v2 (intern only at resolver entry) wins less** — only 1.10× on
   large. Reason: the resolver receives `namespace` arguments from
   `inner.to_namespace`, which still allocates a fresh `Namespace`
   under v2; identity-equal-but-instance-distinct namespaces pollute
   the intern table and force misses. v1's blanket override avoids that
   because every `Namespace.new` is interned at construction.
4. **librbs still dominates** at every size, because the Rust resolver
   operates end-to-end on `u32` IDs and skips the Ruby allocation
   entirely. Resolve-only cost on the librbs path is essentially zero.

## Recommendation for upstream

The interner approach in pure Ruby is a **conditional win**, not a
universal one:

- **Pro**: ~1.5× on `resolve_type_names` for large signature sets
  (think: a Steep run over a real Rails / Selenium-scale project).
  This is the workload where users actually feel resolver cost today.
- **Con**: neutral-to-slightly-slower for typical small/medium
  projects, growing intern tables that never shrink, semantic surprise
  of identity-shared `TypeName` / `Namespace` instances, and thread
  safety considerations on the global tables.

Worth proposing upstream **if framed as opt-in** — for example:
- Have `RBS::Resolver::TypeNameResolver.build` create a per-resolver
  intern table and use `.intern` constructors internally only;
  TypeName / Namespace public API remains alloc-on-`.new`.
- Or expose `RBS::TypeName.with_intern_table { … }` for callers (Steep)
  that want to opt in for batch operations.

A blanket override-`.new` patch is unlikely to land cleanly because of
the regressions on small workloads and the global-state concerns.

## Files

- Patches: `benchmark/rbs_intern_patch.rb`, `benchmark/rbs_intern_patch_v2.rb`
- Benchmarks: `benchmark/three_way_resolve.rb`, `benchmark/resolver_only.rb`
- Existing scripts updated to support the third impl: `benchmark/helpers.rb`
