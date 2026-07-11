# Librbs

TODO: Delete this and the text below, and describe your gem

Welcome to your new gem! In this directory, you'll find the files you need to be able to package up your Ruby library into a gem. Put your Ruby code in the file `lib/librbs`. To experiment with that code, run `bin/console` for an interactive prompt.

## Installation

TODO: Replace `UPDATE_WITH_YOUR_GEM_NAME_IMMEDIATELY_AFTER_RELEASE_TO_RUBYGEMS_ORG` with your gem name right after releasing it to RubyGems.org. Please do not do it earlier due to security reasons. Alternatively, replace this section with instructions to install your gem from git if you don't plan to release to RubyGems.org.

Install the gem and add to the application's Gemfile by executing:

```bash
bundle add UPDATE_WITH_YOUR_GEM_NAME_IMMEDIATELY_AFTER_RELEASE_TO_RUBYGEMS_ORG
```

If bundler is not being used to manage dependencies, install the gem by executing:

```bash
gem install UPDATE_WITH_YOUR_GEM_NAME_IMMEDIATELY_AFTER_RELEASE_TO_RUBYGEMS_ORG
```

## Usage

TODO: Write usage instructions here

## Performance

Comparison of pure RBS vs. librbs on the load + resolve (+ materialize
on librbs) pipeline of a real-world Rails app.

- **Workload**: [kaigionrails/conference-app][ca] — 92 external gems
  wired in through `rbs_collection.lock.yaml` (via
  [gem_rbs_collection][grc], plus rubygems-sourced sigs such as
  `herb`, `reactionview`, `base64`, `bigdecimal`, `prism`) **and** the
  app's own `sig/` directory (161 handwritten and `rbs_rails`-generated
  `.rbs` files).
- **Ruby**: 4.0.2, Ubuntu 24.04, x86_64.
- **Measured region**: everything `rbs list` runs before its print
  loop — `Environment.from_loader(loader).resolve_type_names`, plus
  the librbs-side `Native.materialize_all` triggered by touching
  `env.class_decls`. Type checking and validation are not included.
- **`fast alloc`**: librbs's `obj_alloc + ivar_set` bypass of upstream
  `initialize` funcalls, gated by `LIBRBS_FAST_ALLOC` (default on;
  see `benchmark/README.md`).

[ca]: https://github.com/kaigionrails/conference-app
[grc]: https://github.com/ruby/gem_rbs_collection

### `rbs -Isig list` (CLI wall time)

`hyperfine --warmup 3 --runs 10`, run inside `benchmark/fixtures/`
after the one-shot collection install described in
`benchmark/README.md`. `librbs` is loaded via
`ruby -rlibrbs -e 'load(Gem.bin_path("rbs", "rbs"))'` because the
stock `rbs` executable does not autoload librbs. Times include Ruby
startup and the print loop over `class_decls` / `interface_decls`,
so they are ≈ 1.1 s higher than the isolated load-and-resolve
measurement below.

| variant             | mean ± σ         | min      | speedup vs. pure RBS |
|---------------------|------------------|----------|----------------------|
| pure RBS            | 2.518 s ± 0.085 s | 2.404 s | 1.00x                |
| librbs (normal)     | 2.540 s ± 0.058 s | 2.467 s | 0.99x                |
| librbs (fast alloc) | 1.796 s ± 0.033 s | 1.748 s | **1.40x**            |

### Load + resolve (+ materialize) — script-isolated

`bundle exec ruby benchmark/conference_app_bench.rb`. This wraps the
same `from_loader` + `resolve_type_names` + `class_decls.size` block
in `Benchmark.realtime` inside a subprocess per implementation
(`require "librbs"` patches `RBS::Environment` globally, so each
implementation gets its own process — see `benchmark/helpers.rb`).
Each cell is the minimum of three back-to-back runs inside its
subprocess. Ruby startup and the CLI print loop are excluded.

| variant             | time      | speedup vs. pure RBS |
|---------------------|-----------|----------------------|
| pure RBS            | 1341.6 ms | 1.00x                |
| librbs (normal)     | 1062.7 ms | 1.26x                |
| librbs (fast alloc) | 676.8 ms  | **1.98x**            |

### Reproducing

```sh
# One-shot fixture setup (see benchmark/README.md for details).
cd benchmark/fixtures
BUNDLE_GEMFILE="$PWD/Gemfile" bundle install
bundle exec rbs --collection conference_app.rbs_collection.yaml \
  collection install --frozen
cd -

# Script-isolated numbers (Benchmark.realtime, no Ruby-boot cost).
bundle exec ruby benchmark/conference_app_bench.rb
LIBRBS_FAST_ALLOC=0 bundle exec ruby benchmark/conference_app_bench.rb

# CLI wall time (matches what a real `rbs list` invocation pays).
cd benchmark/fixtures
hyperfine --warmup 3 --runs 10 \
  -n "pure RBS" \
    'bundle exec rbs --collection conference_app.rbs_collection.yaml \
       -I conference_app_sig list > /dev/null' \
  -n "librbs (fast alloc)" \
    'bundle exec ruby -rlibrbs \
       -e "load(Gem.bin_path(%q(rbs), %q(rbs)))" \
       -- --collection conference_app.rbs_collection.yaml \
       -I conference_app_sig list > /dev/null'
```

## Development

After checking out the repo, run `bin/setup` to install dependencies. Then, run `rake spec` to run the tests. You can also run `bin/console` for an interactive prompt that will allow you to experiment.

To install this gem onto your local machine, run `bundle exec rake install`. To release a new version, update the version number in `version.rb`, and then run `bundle exec rake release`, which will create a git tag for the version, push git commits and the created tag, and push the `.gem` file to [rubygems.org](https://rubygems.org).

## Contributing

Bug reports and pull requests are welcome on GitHub at https://github.com/[USERNAME]/librbs.
