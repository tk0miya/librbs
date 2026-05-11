# Benchmark suite

Cold-start benchmarks comparing pure RBS against the librbs native
implementation.

## Setup

```sh
bundle install
bundle exec rake compile     # build the librbs native extension
gem install benchmark-ips    # only needed if you call BenchHelpers.measure_ips
```

### Large-size collection (one-shot)

The `large` size loads gem signatures via an `rbs_collection.lock.yaml`
vendored from [SeleniumHQ/selenium][sel] (33 gems pinned to a specific
[gem_rbs_collection][grc] revision). Populate the local cache once:

```sh
cd benchmark/fixtures
bundle exec rbs --collection selenium.rbs_collection.yaml \
  collection install --frozen
cd -
```

(`--collection` is an option of the top-level `rbs` command, not of
`collection install`, so the flag has to come before the subcommand.)

This clones gem_rbs_collection at the pinned SHA into
`benchmark/fixtures/.gem_rbs_collection/` (gitignored). To swap in a
different OSS project's lockfile, replace
`benchmark/fixtures/selenium.rbs_collection.{yaml,lock.yaml}` and re-run
the install command.

[sel]: https://github.com/SeleniumHQ/selenium/blob/trunk/rb/rbs_collection.lock.yaml
[grc]: https://github.com/ruby/gem_rbs_collection

## Running

Each script drives one workload across two sizes (`small` and `large`)
and both implementations (pure RBS and librbs), then prints a Markdown
table with cold-start wall times and the librbs speedup.

```sh
bundle exec ruby benchmark/load_only.rb
bundle exec ruby benchmark/load_and_resolve.rb
```

The pure-RBS and librbs cases run in **separate Ruby subprocesses** —
`require "librbs"` patches `RBS::Environment` globally and there is no
clean way to undo that in the same process. `helpers.rb` handles the
subprocess plumbing.

### Toggling the `obj_alloc + ivar_set` fast path

librbs's bypass of upstream initializers (currently `Types::Bases::*`)
is gated at runtime by `LIBRBS_FAST_ALLOC`. Default is on; set it to
`0` to fall back to the upstream `Class#new` path. The bench
subprocess inherits this from the parent shell, so a normal-vs-fast
comparison is just two invocations:

```sh
bundle exec ruby benchmark/load_only.rb                       # fast alloc on (default)
LIBRBS_FAST_ALLOC=0 bundle exec ruby benchmark/load_only.rb   # bypass off (normal)
```

## What each script measures

Both scripts run `class_decls.size` at the end so the librbs path
finishes its one-shot `materialize_all` and we are comparing fully
realized Ruby state on both sides. The difference between the two is
purely the cost of `resolve_type_names`.

| script | workload |
|---|---|
| `load_only.rb` | `from_loader` + materialize |
| `load_and_resolve.rb` | `from_loader` + `resolve_type_names` + materialize |

## Sizes

Defined in `helpers.rb` under `BenchHelpers::SIZES`:

- `small` — core only (no extra `loader.add`).
- `large` — core + the gem RBS collection produced by SeleniumHQ/selenium's
  `rbs_collection.lock.yaml` (~33 external gems via gem_rbs_collection).
  Requires the one-shot `rbs collection install` step described above.

## Recording results

Captured tables live under `benchmark/results/`.
