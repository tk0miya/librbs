# frozen_string_literal: true

# Replace `RBS::EnvironmentLoader` with a Librbs-owned class whose
# Loader-internal state (`core_root`, `libs`, `dirs`) lives on the
# Rust side (`Librbs::Native::Loader`). The Ruby class is a thin
# facade — `add(...)`, `each_dir`, `load(env:)` all dispatch to the
# Rust handle.
#
# Two pieces stay Ruby because they're not really Loader logic:
#
# - `@repository` (an `RBS::Repository` instance) is held as a plain
#   Ruby ivar. Rust reads it through funcalls when `each_dir` / `load`
#   need `repository.lookup`. Repository is upstream's class — librbs
#   does not own it (we reverted its Rust port; see
#   `benchmark/summary.md`'s "Repository revert" note).
# - `gem_sig_path` is a class method that wraps RubyGems'
#   `Gem::Specification.find_by_name`. Stays Ruby for the same reason
#   we never tried to Rust-port it.
#
# Two upstream-side concepts are borrowed verbatim:
#
# - `Library` Struct and `UnknownLibraryError` exception are
#   re-exposed via `const_set`. They aren't part of Loader's logic
#   — they're value types that Loader yields / raises — and there's
#   no win to redefining them.
# - `DEFAULT_CORE_ROOT` Pathname constant is borrowed the same way.
#
# Three upstream methods are intentionally **not** implemented:
#
# - `has_library?`, `resolve_dependencies` — only called from inside
#   the loader (verified: no external callers in upstream RBS,
#   librbs tests, or this gem). Their logic is folded into Rust's
#   `each_dir` / `add_lib`.
# - `each_signature` — the only external caller was
#   `spec/compat/gems_spec.rb`, which has been switched to
#   `each_dir`. Implementing `each_signature` in Ruby would
#   reintroduce the upstream `Parser.parse_signature` Ruby AST
#   instantiation path that PR #65 eliminated.
#
# `dirs` reader is also dropped — it has no external callers in any
# of the codebases we audited (upstream / librbs / its specs). The
# Rust handle still tracks `dirs` internally; we just don't expose
# the Ruby reader.

raise "RBS::EnvironmentLoader must be loaded before librbs patches it" unless defined?(RBS::EnvironmentLoader)

_original_loader = RBS::EnvironmentLoader
_original_default_core_root = _original_loader::DEFAULT_CORE_ROOT
_original_library = _original_loader::Library
_original_unknown_library_error = _original_loader::UnknownLibraryError

module Librbs
  module Patches
    class EnvironmentLoader
      def self.gem_sig_path(name, version)
        requirements = []
        requirements << version if version
        spec = Gem::Specification.find_by_name(name, *requirements)
        path = Pathname(spec.gem_dir) + "sig"
        [spec, path] if path.directory?
      rescue Gem::MissingSpecError
        nil
      end

      def initialize(core_root: DEFAULT_CORE_ROOT, repository: RBS::Repository.new)
        @repository = repository
        @__librbs_loader = Librbs::Native::Loader.new(core_root&.to_s)
      end

      attr_reader :repository

      def core_root
        path = @__librbs_loader.core_root
        path ? Pathname(path) : nil
      end

      # Rebuild `Set[Library]` from the Rust state on each call. This
      # is a cold-path reader (upstream callers: `vendorer.rb`, tests)
      # so the per-call allocation is fine; we don't keep a shadow
      # Ruby ivar for it.
      def libs
        set = Set.new
        @__librbs_loader.libs.each do |(name, version)|
          set << self.class::Library.new(name: name, version: version)
        end
        set
      end

      def add(path: nil, library: nil, version: nil, resolve_dependencies: true)
        case
        when path
          @__librbs_loader.add_path(path.to_s)
        when library
          @__librbs_loader.add_lib(library, version, resolve_dependencies)
        end
      end

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

      def each_dir(&block)
        return enum_for(:each_dir) unless block

        @__librbs_loader.each_dir(self, &block)
      end

      # Upstream returns `Array[[decl, path, source]]`; librbs returns
      # `[]` because populating the list would force materialisation of
      # every parsed declaration, defeating `RBS::Environment`'s lazy
      # boundary. Production callers (`Environment.from_loader`,
      # `cli.rb`) discard the return value.
      def load(env:)
        # Mirror upstream's stringio auto-include — keep it in Ruby
        # rather than Rust so the mutation flows through the public
        # `add` API (and surfaces in `libs`) exactly as upstream does.
        if core_root && libs.none? { |lib| lib.name == "stringio" }
          add(library: "stringio", version: nil)
        end
        @__librbs_loader.load(self, env)
        []
      end
    end
  end
end

# Re-expose upstream's constants on the replacement so
# `RBS::EnvironmentLoader::DEFAULT_CORE_ROOT`,
# `RBS::EnvironmentLoader::Library`, and
# `RBS::EnvironmentLoader::UnknownLibraryError` continue to resolve.
Librbs::Patches::EnvironmentLoader.const_set(:DEFAULT_CORE_ROOT, _original_default_core_root)
Librbs::Patches::EnvironmentLoader.const_set(:Library, _original_library)
Librbs::Patches::EnvironmentLoader.const_set(:UnknownLibraryError, _original_unknown_library_error)

RBS.send(:remove_const, :EnvironmentLoader)
RBS.const_set(:EnvironmentLoader, Librbs::Patches::EnvironmentLoader)
