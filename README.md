# librbs — an experimental, Rust-backed accelerator for the RBS loader

`require "librbs"` and RBS type loading gets faster — nothing else
changes. librbs monkey-patches `RBS::EnvironmentLoader` and
`RBS::Environment` with a Rust implementation of the loader hot path:
signature loading (`from_loader`), name resolution
(`resolve_type_names`), and materialization. It is a drop-in accelerator
for RBS- and Steep-based tooling.

> **Experimental.** librbs replaces parts of RBS globally at `require`
> time and is not published to RubyGems. Use it from Git, pinned to a
> commit you have tried.

## Benchmark

Measured with the real `rbs list` command on
[kaigionrails/conference-app][ca] (its own `sig/` plus a 92-gem
collection), Ruby 4.0.4 / rbs 4.0.3, conference-app `899398f`:

| load + resolve (+ materialize) | pure RBS | librbs   | speedup   |
|--------------------------------|----------|----------|-----------|
| conference-app                 | 538.8 ms | 249.3 ms | **2.16x** |

`rbs list` runs exactly the pipeline being measured — load + resolve
(+ materialize on the librbs side), and no type check or validate. See
[`benchmark/`](benchmark/) for the methodology and full breakdown.

## Installation

Not on RubyGems — install from Git. librbs ships a Rust extension, so
building it needs a Rust toolchain (`cargo`).

```ruby
# Gemfile
gem "librbs", git: "https://github.com/tk0miya/librbs.git"
```

Requires Ruby >= 3.3 and `rbs ~> 4.0`.

## Usage

Require librbs once, early — before any RBS environment is built. It
patches RBS in place, so the rest of your process (including any tool
that then loads RBS) uses the accelerated path automatically:

```ruby
require "librbs"   # patches RBS globally; also requires "rbs"
```

For a CLI you don't control, such as `rbs` or `steep`, preload it:

```sh
ruby -rlibrbs -S rbs list       # or: RUBYOPT="-rlibrbs" steep check
```

If the native extension fails to load, librbs warns and falls back to
pure RBS, so behavior is unchanged.

## Development

librbs is a Ruby gem with a Rust extension (via [`rb_sys`][rb_sys] /
magnus), so a Rust toolchain is required to build it.

```sh
bin/setup             # install Ruby dependencies
bundle exec rake      # compile the extension, then run specs and tests
```

The design and the loader hot-path analysis live in
[`docs/design.md`](docs/design.md).

[ca]: https://github.com/kaigionrails/conference-app
[rb_sys]: https://github.com/oxidize-rb/rb-sys
