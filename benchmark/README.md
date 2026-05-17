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
vendored from [kaigionrails/conference-app][ca] (92 gems pinned to a
specific [gem_rbs_collection][grc] revision). Some entries are
`type: rubygems` — their sigs ship inside the gems themselves
(`<gem>/sig/`), so the gems must be installed locally for RBS to find
them. `benchmark/fixtures/Gemfile{,.lock}` is vendored from
conference-app for this; run both setup steps once:

```sh
cd benchmark/fixtures
BUNDLE_GEMFILE="$PWD/Gemfile" bundle install
bundle exec rbs --collection conference_app.rbs_collection.yaml \
  collection install --frozen
cd -
```

(`--collection` is an option of the top-level `rbs` command, not of
`collection install`, so the flag has to come before the subcommand.)

The collection install clones gem_rbs_collection at the pinned SHA into
`benchmark/fixtures/.gem_rbs_collection/` (gitignored). The bundle
install populates the system gem path so `type: rubygems` sigs (herb,
reactionview, base64, bigdecimal, prism) resolve at bench time — the
bench harness strips bundler env from the child process so any locally
installed gem is visible. To swap in a different OSS project's
lockfile, replace `benchmark/fixtures/conference_app.rbs_collection.{yaml,lock.yaml}`
(and `Gemfile{,.lock}` if the gem set differs) and re-run both steps.

[ca]: https://github.com/kaigionrails/conference-app/blob/main/rbs_collection.lock.yaml
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
- `large` — core + the gem RBS collection produced by kaigionrails/conference-app's
  `rbs_collection.lock.yaml` (~92 external gems via gem_rbs_collection).
  Requires the one-shot `rbs collection install` step described above.

## Recording results

Captured tables live in `benchmark/summary.md`.
