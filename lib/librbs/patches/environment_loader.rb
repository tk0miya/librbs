# frozen_string_literal: true

# Replace `RBS::EnvironmentLoader` with `Librbs::Native::EnvironmentLoader`
# (a magnus-wrapped Rust class), without a Ruby facade in between.
# The Loader-internal state (`core_root`, `libs`, `dirs`) lives on the
# pure-Rust `librbs_core::loader::Loader`; the wrapper struct in
# `ext/librbs/src/lib.rs` adds the Ruby-bound bits (`@repository`
# kept as `Opaque<Value>` with GC marking, the `each_dir` block
# protocol, the `RBS::Repository#lookup` callback chain).
#
# This file's job is to:
#   1. Re-expose upstream-side value/exception types on the
#      replacement class via `const_set` (`Library`,
#      `UnknownLibraryError`, `DEFAULT_CORE_ROOT`).
#   2. Add the Ruby methods that wouldn't fit comfortably on the
#      Rust side: the kwargs `new`, the kwargs `add` / `load`
#      dispatchers over the primitive `add_path` / `add_library` /
#      `load_env` methods, `add_collection` (orchestrates over
#      Ruby `Lockfile` / `Repository` / `Collection::Sources`),
#      `gem_sig_path` (RubyGems-bound), and the high-level readers
#      (`core_root` wrapping `Pathname`, `libs` wrapping
#      `Set[Library]`).
#   3. `RBS.const_set(:EnvironmentLoader, ...)` to install the
#      replacement at upstream's name.
#
# Three upstream methods are intentionally not implemented:
#   `has_library?` / `resolve_dependencies` — only ever called
#   from inside upstream `EnvironmentLoader#load`, no external
#   callers in upstream RBS / librbs / its tests. Equivalent logic
#   lives in `each_dir` / `add_library`'s callback chain.
#   `each_signature` — the only external caller (`spec/compat/gems_spec.rb`)
#   was switched to `each_dir`; reimplementing it would reintroduce
#   the upstream `Parser.parse_signature` Ruby AST instantiation
#   that PR #65 eliminated.
#
# The `dirs` reader and the `@libs` / `@dirs` / `@core_root` ivars
# are also dropped — no external callers, and the Rust handle is
# the single source of truth.

raise "RBS::EnvironmentLoader must be loaded before librbs patches it" unless defined?(RBS::EnvironmentLoader)

# Re-expose upstream constants on the replacement class. Done before
# reopening the class so the `new(core_root: DEFAULT_CORE_ROOT, ...)`
# default value can resolve through normal constant lookup.
[
  [:DEFAULT_CORE_ROOT, RBS::EnvironmentLoader::DEFAULT_CORE_ROOT],
  [:Library, RBS::EnvironmentLoader::Library],
  [:UnknownLibraryError, RBS::EnvironmentLoader::UnknownLibraryError]
].each do |name, value|
  Librbs::Native::EnvironmentLoader.const_set(name, value)
end

class Librbs::Native::EnvironmentLoader
  class << self
    # Magnus defines `new(core_root_str, repository)` positionally
    # (see `ext/librbs/src/lib.rs::EnvironmentLoader::new`). Keep it
    # reachable under `__native_new__` so the kwargs wrapper below
    # can call into it.
    alias_method :__native_new__, :new

    def new(core_root: DEFAULT_CORE_ROOT, repository: RBS::Repository.new)
      __native_new__(core_root&.to_s, repository)
    end

    # RubyGems-bound — copied verbatim from upstream because it
    # consults `Gem::Specification.find_by_name` and the surrounding
    # Pathname / Gem::MissingSpecError handling. Same reasoning we
    # keep `add_collection` orchestration on the Ruby side: Ruby is
    # the right language for talking to Ruby objects.
    def gem_sig_path(name, version)
      requirements = []
      requirements << version if version
      spec = Gem::Specification.find_by_name(name, *requirements)
      path = Pathname(spec.gem_dir) + "sig"
      [spec, path] if path.directory?
    rescue Gem::MissingSpecError
      nil
    end
  end

  # Kwargs `add` dispatcher over the primitive `add_path` /
  # `add_library` Rust methods. `add_library` returns the
  # newly-inserted-bool so we can recurse into upstream's
  # `Collection::Sources::*` only on a fresh insertion, matching
  # upstream's `Set#add?`-gated recursion.
  def add(path: nil, library: nil, version: nil, resolve_dependencies: true)
    case
    when path
      add_path(path.to_s)
    when library
      inserted_new = add_library(library, version)
      resolve_dependencies(library: library, version: version) if inserted_new && resolve_dependencies
    end
  end

  # Mirrors upstream `EnvironmentLoader#resolve_dependencies`. Kept
  # in Ruby because the `Collection::Sources::{Rubygems,Stdlib}`
  # singletons are deeply Ruby (manifest YAML parsing, RubyGems
  # spec lookups). Adds discovered dependencies through the public
  # `add` so the same dedup path runs.
  def resolve_dependencies(library:, version:)
    [RBS::Collection::Sources::Rubygems.instance, RBS::Collection::Sources::Stdlib.instance].each do |source|
      next unless source.has?(library, version)

      unless version
        version = source.versions(library).last or raise
      end

      source.dependencies_of(library, version)&.each do |dep|
        add(library: dep["name"], version: nil)
      end
      return
    end
  end

  # Mirrors upstream `EnvironmentLoader#add_collection`. The
  # Lockfile / Repository / Collection::Sources interaction is all
  # Ruby — `add_collection` is orchestration that ends in
  # `self.add(library:)`, which then dispatches to Rust's `add_library`.
  def add_collection(lockfile)
    lockfile.check_rbs_availability!
    repository.add(lockfile.fullpath)

    lockfile.gems.each_value do |gem|
      name = gem[:name]
      locked_version = gem[:version]

      if (source = gem[:source]).is_a?(RBS::Collection::Sources::Rubygems)
        unless source.has?(name, locked_version)
          if (spec, _ = self.class.gem_sig_path(name, nil))
            RBS.logger.warn do
              "Loading type definition from gem `#{name}-#{spec.version}` because locked version " \
                "`#{locked_version}` is unavailable. Try `rbs collection update` to fix the (potential) issue."
            end
            locked_version = spec.version.to_s
          end
        end
      end

      add(library: name, version: locked_version, resolve_dependencies: false)
    end
  end

  # `load_env` is the Rust primitive. Upstream returns
  # `Array[[decl, path, source]]`; librbs returns `[]` because
  # populating that list would force materialisation of every
  # parsed declaration, defeating `RBS::Environment`'s lazy
  # boundary. Production callers (`Environment.from_loader`,
  # `cli.rb`) discard the return value.
  def load(env:)
    # Mirror upstream's stringio auto-include — keep it in Ruby
    # rather than Rust so the mutation flows through the public
    # `add` API (and surfaces in `libs`) exactly as upstream does.
    if core_root && libs.none? { |lib| lib.name == "stringio" }
      add(library: "stringio", version: nil)
    end
    load_env(env)
    []
  end
end

RBS.send(:remove_const, :EnvironmentLoader)
RBS.const_set(:EnvironmentLoader, Librbs::Native::EnvironmentLoader)
