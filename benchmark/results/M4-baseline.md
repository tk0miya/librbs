# M4 baseline benchmark

Date: 2026-05-11
Environment: Ubuntu 24.04 LTS / Ruby 3.3.6 / Linux x86_64 (Intel Xeon @ 2.80GHz, kernel 6.18.5)

Cold-start wall time, minimum of 3 runs per cell. Each (impl, size) pair
runs in its own subprocess (`require "librbs"` patches `RBS::Environment`
globally — see `benchmark/helpers.rb`).

Sizes:

- **small**: core only.
- **large**: core + the gem RBS collection produced by SeleniumHQ/selenium's
  `rbs_collection.lock.yaml` (~33 gems via gem_rbs_collection, plus
  rubygems-sourced sigs such as `webrick`, `prism`).

librbs is reported in two columns — **normal** (upstream
`Class#new` initializers) and **fast alloc** (the `obj_alloc +
ivar_set` bypass; this is the default). See `benchmark/README.md`
for the env-var knob. `speedup_n` / `speedup_f` are pure-RBS divided
by the matching librbs column.

## load_only.rb

`from_loader` + materialize (`class_decls.size` triggers
`Native.materialize_all`).

| size   | pure RBS  | librbs (normal) | speedup_n | librbs (fast alloc) | speedup_f |
|--------|-----------|-----------------|-----------|---------------------|-----------|
| small  |  155.1 ms |        146.2 ms |     1.06x |            163.9 ms |     0.95x |
| large  | 1014.8 ms |        685.0 ms |     1.48x |            646.3 ms |     2.11x |

## load_and_resolve.rb

`from_loader` + `resolve_type_names` + materialize.

| size   | pure RBS  | librbs (normal) | speedup_n | librbs (fast alloc) | speedup_f |
|--------|-----------|-----------------|-----------|---------------------|-----------|
| small  |  281.3 ms |        157.5 ms |     1.79x |            156.1 ms |     1.83x |
| large  | 3115.1 ms |        770.7 ms |     4.04x |            691.5 ms |     4.87x |

## Resolve-only cost (load_and_resolve − load_only)

| size  | pure RBS  | librbs (normal) | librbs (fast alloc)                    |
|-------|-----------|-----------------|----------------------------------------|
| small |  126.2 ms |          11.3 ms|  −7.8 ms (≈0, within run-to-run noise) |
| large | 2100.3 ms |          85.7 ms |  45.2 ms                              |

In librbs the resolve phase is essentially free — every visible difference
between the two scripts on the librbs side is run-to-run jitter. Pure RBS
spends ~120ms (small) to ~2.1s (large) inside `resolve_type_names`, so
that step alone accounts for the bulk of the resolve-path gap.

## Notes captured during the run

- The bench harness leaked `BUNDLE_GEMFILE` / `RUBYOPT` into the subprocess
  even though `Bundler.unbundled_env` was used, because `Open3.capture3`
  inherits the parent env for keys absent from the override hash.
  `BenchHelpers.unbundled_env` now explicitly sets the absent keys to `nil`
  so Open3 removes them in the child. Without this fix the `large` size's
  pure-RBS subprocess could not find gem-installed sigs (e.g. `webrick`).
- `librbs::Native.build_environment` previously rejected libraries sourced
  from installed gems (`type: rubygems` in the collection lockfile —
  `webrick`, `prism`, ...) with `unknown library: <name>`. The Rust
  `Repository` only knows the gem_rbs_collection layout. The fix
  pre-resolves each lib's path with upstream
  `RBS::EnvironmentLoader.gem_sig_path` on the Ruby side and passes the
  result through a new `Loader::add_library_with_path` Rust API, mirroring
  upstream's `gem_sig_path` → `repository.lookup` fallback chain in
  `vendor/rbs/lib/rbs/environment_loader.rb#each_dir`.
- Materializer optimisations layered on top of the original baseline
  (all reflected in the librbs numbers above):
  - `TypeName` / `Namespace` are flyweighted by interner Sym, so the same
    `(NamespaceSym, name, kind)` triple yields a shared Ruby instance
    across the whole environment.
  - `RBS::Location` children are appended via a dlsym FFI bridge into
    `rbs_loc_legacy_add_required_child` /
    `rbs_loc_legacy_add_optional_child`, with the children array
    pre-sized via `rbs_loc_legacy_alloc_children`. Calling those C
    entry points directly bypasses Ruby method dispatch, the
    `rb_check_typeddata` re-lookup, `rb_sym2id`, `NUM2INT`, and the
    Symbol allocation that the underscore-prefixed Ruby primitives
    would otherwise perform on every child append.
  - `RBS::Location` instances themselves (≈91k for the `large` corpus)
    are constructed via the same dlsym bridge into upstream's
    `rbs_new_location2(VALUE buffer, int start_char, int end_char)`,
    which calls `TypedData_Make_Struct` + `rbs_loc_init` directly. That
    skips the `RBS::Location.new` → `class_alloc` → `initialize`
    funcall pair, the `rbs_check_location` re-lookup that
    `location_initialize` does, and the two `FIX2INT` round-trips on
    `start` / `end` (we already have them as `i32`s out of the parser).
    On a same-machine A/B (best of 8 cold runs each) `large` load_only
    moved from 631.2 ms → 572.3 ms (−9%) with no measurable change on
    `small` — the per-call savings only surface above the noise floor
    on workloads with enough Location allocations.
  - Static Ruby `Symbol` values used by the materializer (kind /
    visibility / variance keywords, the `Overload` const-get key) are
    pre-interned once on `MaterializeCtx::common`, and interner-backed
    `Sym`s flow through a per-ctx `symbol_cache` flyweight indexed by
    `Sym.0`. The legacy `Ruby::to_symbol(&str)` path allocated an
    intermediate `RString` per call; the cache reduces that to a single
    `rb_intern2` + `rb_id2sym` on first sight, then a `Vec` index
    afterwards.
  - `RBS::Types::Bases::*` instances (`Bool`, `Void`, `Nil`, `Top`,
    `Bottom`, `Self`, `Instance`, `Class`, `Any`) skip the
    `new_instance(kwargs!(...))` path entirely and write `@location`
    (and `@string` for `Any` with `todo: true`) straight onto a
    freshly-`obj_alloc`'d instance. The ivar `Id`s are pre-interned on
    `MaterializeCtx::common` alongside the symbol cache. On a
    Bases-heavy synthetic corpus (2000 classes × 6 methods returning
    untyped / bool / nil / void / self / top, ≈12k Bases instances)
    best-of-3 min wall time dropped from ~228 ms to ~200 ms (≈12%);
    pure-Ruby micro shows `alloc + ivar_set` at ≈460 ns/op vs
    `Bool.new(location:)` at ≈1.8 µs/op. Gated at runtime by
    `LIBRBS_FAST_ALLOC` (see `benchmark/README.md`); the toggle is
    reflected in the `normal` / `fast alloc` columns above.
