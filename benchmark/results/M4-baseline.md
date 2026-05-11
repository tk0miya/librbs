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

| size   | pure RBS  | librbs    | speedup |
|--------|-----------|-----------|---------|
| small  | 154.1 ms  | 186.6 ms  | 0.83x   |
| medium | 186.5 ms  | 197.1 ms  | 0.95x   |
| large  | 1109.6 ms | 739.8 ms  | 1.50x   |

## load_and_resolve.rb

`from_loader` + `resolve_type_names` + materialize.

| size   | pure RBS  | librbs    | speedup |
|--------|-----------|-----------|---------|
| small  | 269.5 ms  | 171.6 ms  | 1.57x   |
| medium | 359.1 ms  | 235.9 ms  | 1.52x   |
| large  | 2723.9 ms | 725.5 ms  | 3.75x   |

## Resolve-only cost (load_and_resolve − load_only)

| size   | pure RBS  | librbs    |
|--------|-----------|-----------|
| small  | 115.4 ms  | −15.0 ms (≈0, within run-to-run noise) |
| medium | 172.6 ms  |  38.8 ms  |
| large  | 1614.3 ms | −14.3 ms (≈0, within run-to-run noise) |

In librbs the resolve phase is essentially free — every visible difference
between the two scripts on the librbs side is run-to-run jitter. Pure RBS
spends ~115ms (small) to ~1.6s (large) inside `resolve_type_names`, so
that step alone accounts for ~2.2x (large) and ~1.5x (medium) of the
gap.

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
