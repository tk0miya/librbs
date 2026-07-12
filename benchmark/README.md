# Benchmark suite

Benchmarks comparing pure RBS against the librbs native implementation.
There are two, both on the [kaigionrails/conference-app][ca] workload:

- **`benchmark.rb`** — an in-process microbenchmark of
  `from_loader` + `resolve_type_names` + materialize (gem collection
  only). See [Running](#running).
- **`list_benchmark.sh`** — times the real `rbs -Isig list` command on
  the full app (its own `sig/` *and* the gem collection). See
  [`rbs list` command comparison](#rbs-list-command-comparison-ruby-40).

## Setup

```sh
bundle install
bundle exec rake compile     # build the librbs native extension
gem install benchmark-ips    # only needed if you call BenchHelpers.measure_ips
```

### Workload (one-shot)

Both benchmarks run on [kaigionrails/conference-app][ca]. This repo
**does not vendor any of it** — the project is cloned (pinned) into
`benchmark/fixtures/conference-app/` (gitignored) by the steps below,
so nothing from another repository lives in this tree. Both benchmarks
read the clone directly: its `rbs_collection.lock.yaml` (92 gems pinned
to a [gem_rbs_collection][grc] revision) and, for `list_benchmark.sh`,
its `sig/`.

A blobless, sparse clone keeps only what the benchmarks touch (`sig/` +
the collection/Gemfile files, ~160 sig files, no app source):

```sh
cd benchmark/fixtures
git clone --filter=blob:none --sparse https://github.com/kaigionrails/conference-app.git
cd conference-app
git sparse-checkout set --no-cone /sig /rbs_collection.yaml /rbs_collection.lock.yaml /Gemfile /Gemfile.lock
BUNDLE_GEMFILE="$PWD/Gemfile" bundle install
rbs collection install --frozen
cd -
```

Everything after `bundle install` runs inside the clone, where `rbs`
auto-discovers `rbs_collection.yaml` by its standard name — no
`--collection` flag needed, for either the install here or the
benchmarks below.

The clone tracks conference-app's default branch — the benchmarks run
against **whatever it points at today**, deliberately not pinned, so
results reflect a current real-world app. Reproducibility instead lives
with each result: record the commit you measured
(`git -C benchmark/fixtures/conference-app rev-parse --short HEAD`)
next to its numbers, as the tables below do.

The collection install clones gem_rbs_collection at the pinned SHA into
`conference-app/.gem_rbs_collection/`. The `bundle install` (on the
app's own Gemfile) populates the system gem path so `type: rubygems`
sigs (herb, reactionview, base64, bigdecimal, prism) resolve at bench
time — the bench harnesses strip bundler env from the child process so
any locally installed gem is visible. To pin a different revision, just
change the `git checkout` SHA and re-run collection install.

[ca]: https://github.com/kaigionrails/conference-app
[grc]: https://github.com/ruby/gem_rbs_collection

## Running

The bench drives one workload — `from_loader` + `resolve_type_names`
+ materialize, the full "give me a usable RBS::Environment" pipeline
— against both implementations (pure RBS and librbs), then prints a
Markdown table with wall times and the librbs speedup.
`class_decls.size` at the end of the timed block forces the librbs
path's one-shot `materialize_all` so we are comparing fully realized
Ruby state on both sides.

```sh
bundle exec ruby benchmark/benchmark.rb
```

The pure-RBS and librbs cases run in **separate Ruby subprocesses** —
`require "librbs"` patches `RBS::Environment` globally and there is no
clean way to undo that in the same process. `helpers.rb` handles the
subprocess plumbing.

### Toggling the `obj_alloc + ivar_set` fast path

librbs's bypass of upstream initializers is gated at runtime by
`LIBRBS_FAST_ALLOC`. Default is on; set it to `0` to fall back to
the upstream `Class#new` path. The bench subprocess inherits this
from the parent shell, so a normal-vs-fast comparison is just two
invocations:

```sh
bundle exec ruby benchmark/benchmark.rb                       # fast alloc on (default)
LIBRBS_FAST_ALLOC=0 bundle exec ruby benchmark/benchmark.rb   # bypass off (normal)
```

## Workload

Core signatures + the gem RBS collection produced by
kaigionrails/conference-app's `rbs_collection.lock.yaml` (~92 external
gems via gem_rbs_collection). Requires the one-shot [Workload
setup](#workload-one-shot) above. See `BenchHelpers::COLLECTION_LOCKFILE`
in `helpers.rb` for the exact path.

## `rbs list` command comparison (Ruby 4.0)

`benchmark.rb` above measures the load pipeline in-process. This second
benchmark instead times the **real `rbs list` command**, because that is
the closest off-the-shelf tool to "load a project's types, names
resolved, and nothing more". `RBS::CLI#run_list` does exactly:

```ruby
env = Environment.from_loader(loader).resolve_type_names   # load + resolve
env.class_decls.each { ... }                               # iterate
env.class_alias_decls.each { ... }
env.interface_decls.each { ... }
```

and nothing else — no type check, no validate. Under librbs the
`class_decls` iteration triggers the one-shot `materialize_all`, so a
single `rbs list` invocation covers **load + resolve (+ materialize for
librbs)**, which is precisely the target.

### Workload

[kaigionrails/conference-app][ca] carries both external and
first-party types, so it exercises the whole loader (from the clone set
up in [Workload (one-shot)](#workload-one-shot)):

- its own application signatures — 161 `.rbs` files under the clone's
  `sig/`, passed via `-I`;
- 92 external gems via the clone's `rbs_collection.lock.yaml`,
  auto-discovered from `rbs_collection.yaml` (same collection as
  `benchmark.rb`).

Run from inside the clone, the command is just what a conference-app
developer would type — `rbs_collection.yaml` is picked up by its
standard name, so no `--collection` flag:

```sh
cd benchmark/fixtures/conference-app
rbs -I sig list
```

For librbs, `librbs` is preloaded (`ruby -rlibrbs -S rbs …`) so it
patches `RBS` before the CLI builds the environment. Both cases resolve
the same 4236 entities (2985 classes, 1130 modules, 109 interfaces, 12
aliases) — verified identical output.

### Running

```sh
RBENV_VERSION=4.0.4 benchmark/list_benchmark.sh          # RUNS=20 by default
RUNS=25 RBENV_VERSION=4.0.4 benchmark/list_benchmark.sh
```

Same one-shot prerequisites as `benchmark.rb` (`rake compile` for the
target Ruby + the collection install above). The script strips bundler
env so the child sees the full local gemset — the collection's
`type: rubygems` sigs (herb, prism, base64, …) ship inside the gems
themselves and must be resolvable. `list_driver.rb` times each command
end-to-end (VM boot + `require` + work) and reports min/median/mean.

### Results

Ruby 4.0.4, rbs 4.0.3, macOS 15 (arm64-darwin24, M4 Mac),
conference-app `899398f`. Wall time, min-of-25:

| stage                                   | pure RBS  | librbs    | speedup |
|-----------------------------------------|-----------|-----------|---------|
| `rbs -Isig list` (end-to-end)           | 791.3 ms  | 504.7 ms  | 1.57x   |
| startup baseline (`rbs version`)        | 252.5 ms  | 255.4 ms  | —       |
| **load + resolve (+ materialize)**      | **538.8 ms** | **249.3 ms** | **2.16x** |

The end-to-end row is what a caller actually pays for the command. Since
that includes a fixed startup cost (VM boot + `require "rbs"`) that is
*not* load/resolve work, it is measured separately with `rbs version`
(≈253 ms, essentially equal for both — librbs's native extension adds
only ~3 ms). Subtracting it isolates the measurement target: librbs cuts
load + resolve + materialize from 538.8 ms to 249.3 ms, a **2.16×
speedup**.

## Recording results

Captured tables live in `benchmark/summary.md`.
