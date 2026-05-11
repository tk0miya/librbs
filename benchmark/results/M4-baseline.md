# M4 baseline benchmark

Date: 2026-05-11
Environment: Ubuntu 24.04 LTS / Ruby 3.3.6 / Linux x86_64 (Intel Xeon @ 2.80GHz, kernel 6.18.5)

Cold-start wall time, minimum of 3 runs per cell. Each (impl, size) pair
runs in its own subprocess (`require "librbs"` patches `RBS::Environment`
globally — see `benchmark/helpers.rb`).

Sizes:

- **small**: core only.
- **medium**: core + `pathname date time uri optparse logger stringio strscan`.
- **large**: core + the gem RBS collection produced by SeleniumHQ/selenium's
  `rbs_collection.lock.yaml` (~33 gems via gem_rbs_collection, plus
  rubygems-sourced sigs such as `webrick`, `prism`).

## load_only.rb

`from_loader` + materialize (`class_decls.size` triggers
`Native.materialize_all`).

| size   | pure RBS | librbs   | speedup |
|--------|----------|----------|---------|
| small  | 114.1 ms | 110.9 ms | 1.03x   |
| medium | 152.7 ms | 125.1 ms | 1.22x   |
| large  | 772.5 ms | 503.4 ms | 1.53x   |

## load_and_resolve.rb

`from_loader` + `resolve_type_names` + materialize.

| size   | pure RBS  | librbs   | speedup |
|--------|-----------|----------|---------|
| small  |  218.6 ms | 119.7 ms | 1.83x   |
| medium |  292.6 ms | 145.9 ms | 2.00x   |
| large  | 1999.6 ms | 469.5 ms | 4.26x   |

## Resolve-only cost (load_and_resolve − load_only)

| size   | pure RBS  | librbs                                 |
|--------|-----------|----------------------------------------|
| small  |  104.5 ms |   8.8 ms                               |
| medium |  139.9 ms |  20.8 ms                               |
| large  | 1227.1 ms | −33.9 ms (≈0, within run-to-run noise) |

In librbs the resolve phase is essentially free — every visible difference
between the two scripts on the librbs side is run-to-run jitter. Pure RBS
spends ~100ms (small) to ~1.2s (large) inside `resolve_type_names`, so
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
    `Bool.new(location:)` at ≈1.8 µs/op.
