# M4 baseline benchmark

Date: 2026-05-17
Environment: macOS 15 / Ruby 3.3.11, 3.4.9, 4.0.4 (rbenv-switched) / Darwin 24.6.0 (arm64-darwin24, M4 Mac)

Cold-start wall time, minimum of 3 runs per cell. Each (impl, size, ruby)
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

## load_only.rb

`from_loader` + materialize (`class_decls.size` triggers
`Native.materialize_all`).

| ruby   | size  | pure RBS  | librbs (normal) | speedup_n | librbs (fast alloc) | speedup_f |
|--------|-------|-----------|-----------------|-----------|---------------------|-----------|
| 3.3.11 | small |   48.4 ms |         41.0 ms |     1.18x |             27.2 ms |     1.78x |
| 3.3.11 | large | 1062.4 ms |        468.5 ms |     2.27x |            219.7 ms |     4.84x |
| 3.4.9  | small |   40.6 ms |         35.9 ms |     1.13x |             27.1 ms |     1.50x |
| 3.4.9  | large |  261.7 ms |        342.8 ms |     0.76x |            202.3 ms |     1.29x |
| 4.0.4  | small |   33.3 ms |         34.5 ms |     0.97x |             22.3 ms |     1.49x |
| 4.0.4  | large |  227.6 ms |        325.6 ms |     0.70x |            171.7 ms |     1.33x |

## load_and_resolve.rb

`from_loader` + `resolve_type_names` + materialize.

| ruby   | size  | pure RBS  | librbs (normal) | speedup_n | librbs (fast alloc) | speedup_f |
|--------|-------|-----------|-----------------|-----------|---------------------|-----------|
| 3.3.11 | small |   89.5 ms |         42.4 ms |     2.11x |             26.4 ms |     3.39x |
| 3.3.11 | large | 2115.8 ms |        431.8 ms |     4.90x |            219.7 ms |     9.63x |
| 3.4.9  | small |   58.4 ms |         37.9 ms |     1.54x |             23.5 ms |     2.49x |
| 3.4.9  | large |  514.4 ms |        342.8 ms |     1.50x |            213.7 ms |     2.41x |
| 4.0.4  | small |   51.4 ms |         33.0 ms |     1.56x |             22.9 ms |     2.24x |
| 4.0.4  | large |  391.9 ms |        260.6 ms |     1.50x |            173.0 ms |     2.27x |

## Cross-Ruby observations

- Pure RBS gets dramatically faster on 3.4+. `large` `load_only` drops
  from ~1060 ms on 3.3.11 to ~260 ms on 3.4.9 and ~230 ms on 4.0.4 —
  Ruby 3.4 makes Prism the default parser, and the same effect carries
  into `resolve_type_names` (large pure RBS resolve cost falls from
  ~1050 ms on 3.3.11 to ~250 ms / ~165 ms on 3.4 / 4.0).
- librbs **normal** mode now loses to pure RBS on 3.4+ `large`
  `load_only` (0.76x / 0.70x). The materializer's `:initialize` funcalls
  cost more than what librbs saves on the parser side once pure RBS has
  Prism. The 3.3.11 numbers (2.27x normal) are no longer representative
  of the upstream path on current Ruby — the **fast alloc** bypass is
  the only librbs config that still wins everywhere.
- **Fast alloc** retains a clear margin on every cell: small 1.49x–1.78x
  (load_only) / 2.24x–3.39x (load_and_resolve), large 1.29x–4.84x /
  2.27x–9.63x. The 3.3.11 large `load_and_resolve` 9.63x is the high
  watermark; on 3.4+ it compresses to ~2.3x because pure RBS shrank,
  not because librbs slowed down (fast-alloc large `load_only` only
  drops from 220 ms to ~170 ms across the three Rubies).
- The resolve phase remains essentially free in librbs across every
  Ruby (`load_and_resolve` − `load_only` is within run-to-run noise for
  both normal and fast columns), so the speedup compression on 3.4+ is
  entirely explained by pure RBS getting faster, not by librbs
  regressing.

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
