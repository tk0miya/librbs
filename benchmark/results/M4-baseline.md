# M4 baseline benchmark

Date: 2026-05-10
Environment: Ubuntu 24.04 LTS / Ruby 3.3.6 / Linux x86_64 (Intel Xeon @ 2.80GHz, kernel 6.18.5)

Cold-start wall time, minimum of 5 runs per cell. Each (impl, size) pair
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
| small  | 146.3 ms | 180.4 ms | 0.81x   |
| medium | 177.9 ms | 247.3 ms | 0.72x   |
| large  | 867.2 ms | 891.7 ms | 0.97x   |

## load_and_resolve.rb

`from_loader` + `resolve_type_names` + materialize.

| size   | pure RBS  | librbs   | speedup |
|--------|-----------|----------|---------|
| small  |  259.6 ms | 184.6 ms | 1.41x   |
| medium |  382.6 ms | 226.5 ms | 1.69x   |
| large  | 2448.5 ms | 870.3 ms | 2.81x   |

## Resolve-only cost (load_and_resolve − load_only)

| size   | pure RBS  | librbs                                |
|--------|-----------|---------------------------------------|
| small  |  113.3 ms |   4.2 ms                              |
| medium |  204.7 ms | −20.8 ms (≈0, within run-to-run noise)|
| large  | 1581.3 ms | −21.4 ms (≈0, within run-to-run noise)|

In librbs the resolve phase is essentially free — every visible difference
between the two scripts on the librbs side is run-to-run jitter. Pure RBS
spends ~110ms (small) to ~1.6s (large) inside `resolve_type_names`, so
that step alone accounts for the bulk of the load-and-resolve speedup.

## Materialize-only timing (focused)

Cold-start cells include Ruby boot, parser load, and stdlib require, so
the materialize improvement is partially hidden by subprocess noise. The
numbers below come from a single in-process run (`require "rbs"; require
"librbs"`), 30 timed iterations per cell with `GC.start` before each;
only the `class_decls.size` call is timed (which triggers
`Native.materialize_all`). `from_loader` is excluded.

| size   | materialize median |
|--------|--------------------|
| small  |  181 ms            |
| medium |  219 ms            |
| large  |  931 ms            |

Materialize is still the largest single component of librbs's load-only
wall time on every size; see the "Pointers" section of
`M4-decision.md` for the remaining hot spots.

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
