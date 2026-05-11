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
| small  | 111.4 ms | 116.0 ms | 0.96x   |
| medium | 142.5 ms | 142.2 ms | 1.00x   |
| large  | 725.3 ms | 486.0 ms | 1.49x   |

## load_and_resolve.rb

`from_loader` + `resolve_type_names` + materialize.

| size   | pure RBS  | librbs   | speedup |
|--------|-----------|----------|---------|
| small  | 208.0 ms  | 122.4 ms | 1.70x   |
| medium | 276.7 ms  | 132.9 ms | 2.08x   |
| large  | 1833.1 ms | 476.4 ms | 3.85x   |

## Resolve-only cost (load_and_resolve − load_only)

| size   | pure RBS  | librbs                                |
|--------|-----------|---------------------------------------|
| small  |  96.6 ms  |  6.4 ms                               |
| medium | 134.2 ms  | −9.3 ms (≈0, within run-to-run noise) |
| large  | 1107.8 ms | −9.6 ms (≈0, within run-to-run noise) |

In librbs the resolve phase is essentially free — every visible difference
between the two scripts on the librbs side is run-to-run jitter. Pure RBS
spends ~100ms (small) to ~1.1s (large) inside `resolve_type_names`, so
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
- Two materialiser optimisations landed after the original M4 baseline
  was recorded and are reflected in the librbs numbers above:
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
