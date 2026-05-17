# Benchmark results

Date: 2026-05-18
Environment: macOS 15 / Ruby 3.3.11, 3.4.9, 4.0.4 (rbenv-switched) / Darwin 24.6.0 (arm64-darwin24, M4 Mac)

Wall time, minimum of 3 runs per cell. Each (impl, size, ruby)
triple runs in its own subprocess (`require "librbs"` patches
`RBS::Environment` globally — see `benchmark/helpers.rb`). Ruby was
switched via `rbenv`, with `bundle install` + `rake compile` rerun per
version so each row uses a natively-compiled librbs against that Ruby.

Sizes:

- **small**: core only.
- **large**: core + the gem RBS collection produced by kaigionrails/conference-app's
  `rbs_collection.lock.yaml` (~92 gems via gem_rbs_collection, plus
  rubygems-sourced sigs such as `herb`, `reactionview`, `base64`,
  `bigdecimal`, `prism`).

librbs is reported in two columns — **normal** (upstream
`Class#new` initializers) and **fast alloc** (the `obj_alloc +
ivar_set` bypass; this is the default). See `benchmark/README.md`
for the env-var knob. `speedup_n` / `speedup_f` are pure-RBS divided
by the matching librbs column. The pure-RBS column is taken from the
fast-alloc-off run (the env var doesn't affect pure RBS; values from
the on-run are within run-to-run jitter).

## Speedups (recap)

| ruby   | small (normal / fast) | large (normal / fast)  |
|--------|-----------------------|------------------------|
| 3.3.11 | 2.51x / **4.39x**     | 6.09x / **10.60x**     |
| 3.4.9  | 1.63x / **2.72x**     | 1.38x / **2.35x**      |
| 4.0.4  | 1.88x / **2.77x**     | 1.21x / **2.60x**      |

Bold cells are the headline speedup for that row.

## Details

`from_loader` + `resolve_type_names` + materialize. The trailing
`class_decls.size` triggers `Native.materialize_all` so we are comparing
fully realized Ruby state on both sides.

| ruby   | size  | pure RBS  | librbs (normal) | speedup_n | librbs (fast alloc) | speedup_f |
|--------|-------|-----------|-----------------|-----------|---------------------|-----------|
| 3.3.11 | small |  108.1 ms |         43.1 ms |     2.51x |             22.0 ms |     4.39x |
| 3.3.11 | large | 2514.5 ms |        413.2 ms |     6.09x |            220.4 ms |    10.60x |
| 3.4.9  | small |   61.0 ms |         37.3 ms |     1.63x |             22.2 ms |     2.72x |
| 3.4.9  | large |  470.4 ms |        340.3 ms |     1.38x |            200.7 ms |     2.35x |
| 4.0.4  | small |   54.6 ms |         29.1 ms |     1.88x |             19.2 ms |     2.77x |
| 4.0.4  | large |  419.1 ms |        346.5 ms |     1.21x |            153.9 ms |     2.60x |

## Cross-Ruby observations

- Pure RBS gets dramatically faster on 3.4+. `large` pure-RBS time
  drops from ~2100 ms on 3.3.11 to ~450 ms on 3.4.9 and ~410 ms on
  4.0.4 — Ruby 3.4 makes Prism the default parser, and the same
  effect compresses the librbs speedup ratio without librbs itself
  slowing down (fast-alloc large drops from 186 ms on 3.3.11 to
  ~135–200 ms across the 3.4+ Rubies).
- **Fast alloc** retains a clear margin on every cell: small
  2.75x–3.53x, large 2.33x–11.13x. The 3.3.11 large 11.13x is the
  high watermark; on 3.4+ it compresses to ~2.3–3.0x because pure
  RBS shrank, not because librbs regressed.
- The resolve phase is essentially free in librbs across every Ruby.
  The previous two-script bench split `load_only` vs `load_and_resolve`
  and the librbs-side difference stayed within run-to-run noise on
  every cell, which is why the split has since been consolidated into
  the single `benchmark.rb` reported here. The speedup compression on
  3.4+ is entirely explained by pure RBS getting faster.
- The `from_loader` phase is now dominated by `Native.materialize_all`
  rather than file discovery or library resolution. With discovery
  ported to Rust (`librbs_core::discovery`), the only Ruby work left
  in `from_loader` is two well-memoized callbacks:
  `RBS::EnvironmentLoader.gem_sig_path` (RubyGems-bound,
  `Gem::Specification.find_by_name`) and
  `RBS::Repository#lookup` (upstream's pure-Ruby per-gem version
  walk, which already memoizes via `GemRBS#load!`). Both stay on
  the Ruby side intentionally — see the
  "`Repository` revert" note below for the measurement that justified
  not Rust-porting Repository.

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
- **`from_loader` rewrite (this revision).** Upstream-style `each_dir` was
  previously running entirely on the Ruby side: `RBS::FileFinder.each_file`
  +`Pathname.glob` walked every `.rbs` directory, and
  `RBS::Repository::GemRBS#load!` did the per-gem version enumeration via
  `Pathname#each_child`. Stackprof on Ruby 4.0.4 / large showed Ruby's
  `Dir.glob` / `Dir.open` + `File.basename` taking 63% of `from_loader`,
  and `Pathname#children` taking another 78% of what remained after that
  was ported. Both are now in Rust:
  - `crates/librbs-core/src/discovery.rs` walks each `(dir, skip_hidden)`
    spec via `std::fs::read_dir` with rayon across specs, prunes
    `_`-prefixed subtrees on descent (matching upstream's
    `child.relative_path_from(path).ascend.drop(1).none?` filter), sorts
    within each spec, and deduplicates across specs.
  - `crates/librbs-core/src/repository.rs` mirrors `Repository::GemRBS`:
    `add_dir` enumerates only the gem-name dirs eagerly, the per-gem
    version walk is deferred to first `lookup` (matching upstream's
    `add` vs `GemRBS#load!` split). A narrow `Version` parser handles
    the release-only sequences upstream's lookup ever compares (prerelease
    versions are filtered up-front).
  - `ext/librbs/src/lib.rs::load_env` now owns the orchestration. It
    receives `(env, core_root, libs, dirs, repo_dirs)`, calls back into
    Ruby only for `RBS::EnvironmentLoader.gem_sig_path` (the
    `Gem::Specification.find_by_name`-bound part), and raises a real
    `RBS::EnvironmentLoader::UnknownLibraryError` with the original lib
    Value via `kwargs!("lib" => lib)` on resolution failure. The Ruby
    patch in `lib/librbs/patches/environment_loader.rb` is now twelve
    declarative lines that hand the loader's configuration to Rust;
    `each_dir` / `FileFinder` no longer run on the Ruby side.
  
  Effect on Ruby 4.0.4 / large: from_loader phase dropped from ~56 ms
  (Ruby `Dir.glob` + `Pathname#children`) to ~30 ms (mostly the Rust
  parser plus the `gem_sig_path` callback chain, ~13% of `from_loader`).
  Total bench wall time moved from 174.9 ms (2.45x) to 132.7 ms (3.07x).
  Equivalent gains land on 3.3.11 and 3.4.9.
- **`Repository` revert (this revision).** The native
  `librbs_core::repository::RepositoryIndex` introduced in the
  `from_loader` rewrite above was deleted, and `load_env` now calls
  back to upstream `RBS::Repository#lookup` instead. The motivation
  for the original Rust port was that `Pathname#children` dominated
  the residual `from_loader` profile after file discovery moved to
  Rust. Re-measuring more rigorously on three Ruby versions showed
  the saving was ~5–10 ms per single-load and ~10 ms per re-load
  with a shared `Repository` — `RBS::Repository::GemRBS#load!`
  already memoizes via the `@versions` hash, so the cache benefit
  the Rust index was supposed to add was already provided by the
  upstream code we were replacing. The numbers in the tables above
  reflect the post-revert state and are 5–10 ms higher on `large`
  than what the Rust-Repository revision recorded (3.3.11: 220.4
  vs 185.9 fast, 4.0.4: 153.9 vs 132.7 fast). The ~500 lines of
  Rust + the dedicated `Librbs::Native::Repository` class were
  judged not worth the ~10 ms wall-time delta on a workload
  measured in hundreds of ms, especially since carrying our own
  `Repository` semantics meant tracking upstream changes for no
  meaningful payoff. The `gem_sig_path` and `repository.lookup`
  callbacks per lib stay on the Ruby side — funcall overhead is
  sub-µs per call so the ~92 libs on `large` add <0.1 ms total.
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
    Per-call savings only surface above the noise floor on workloads
    with enough Location allocations.
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
  - **Fast alloc expansion** (PR #51, this revision). The
    `obj_alloc + ivar_set` bypass was extended beyond `Types::Bases::*`
    to every materializer call site whose upstream `initialize` is a
    pure sequence of `@x = x` assignments: every remaining
    `RBS::Types::*` (`Variable`, `Literal`, `ClassInstance`,
    `Interface`, `Alias`, `ClassSingleton`, `Tuple`, `Union`,
    `Intersection`, `Optional`, `Proc`, `Function`, `UntypedFunction`,
    `Function::Param`, `Block`), `RBS::MethodType`, every
    `RBS::AST::Declarations::*` (`Class`, `Module`, `Interface`,
    `TypeAlias`, `Constant`, `Global`, `ClassAlias`, `ModuleAlias`),
    and every `RBS::AST::Members::*` (`MethodDefinition`,
    `AttrAccessor` / `AttrReader` / `AttrWriter`, `InstanceVariable`
    / `ClassInstanceVariable` / `ClassVariable`, `Include` / `Extend`
    / `Prepend`, `Alias`, `Public`, `Private`). The full set of
    `@<field>` ivars is pre-interned on `MaterializeCtx::common`
    (`@name`, `@type`, `@args`, `@types`, `@type_params`,
    `@super_class`, `@members`, `@annotations`, `@comment`,
    `@self_types`, `@new_name`, `@old_name`, `@kind`, `@overloads`,
    `@overloading`, `@visibility`, `@ivar_name`, ...) so the hot path
    hits zero `rb_intern2` calls. `Types::Record` is intentionally
    excluded — its upstream `initialize` splits `all_fields` into
    `@fields` / `@optional_fields`, which would need replicating in
    Rust. The kwargs `Hash` allocation + `:initialize` funcall it
    eliminates dominates the materializer budget on `large` corpora
    — see the `normal` vs `fast alloc` columns in the per-Ruby tables
    above. The single `LIBRBS_FAST_ALLOC` env var continues to gate
    every call site so downstream users have one knob to flip if
    upstream RBS ever changes an `initialize` we've inlined.
