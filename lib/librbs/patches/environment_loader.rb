# frozen_string_literal: true

module Librbs
  module Patches
    module EnvironmentLoader
      # Replace upstream `RBS::EnvironmentLoader#load`. The Ruby patch
      # collects the loader's configuration and hands it to
      # `Librbs::Native.load_env`, which owns the rest of the
      # `each_dir` orchestration: gem resolution (with a Ruby callback
      # into `RBS::EnvironmentLoader.gem_sig_path` for the RubyGems-
      # bound part), repository version-best lookup (native via
      # `librbs_core::repository::RepositoryIndex`), file discovery,
      # read + parse, and `Environment::add_source`.
      #
      # `libs` is passed as `Array<Library>` rather than a list of
      # `(name, version)` tuples so the native side can attach the
      # original lib Value to `RBS::EnvironmentLoader::UnknownLibraryError`
      # via `lib:` — matching upstream's exception shape exactly.
      #
      # The returned Array is intentionally empty. Upstream returns a
      # `[[decl, path, source], ...]` list, but populating it would
      # force materialisation of every parsed declaration — defeating
      # the lazy boundary `RBS::Environment#class_decls` etc. rely on.
      # Production callers (`Environment.from_loader`, `cli.rb`)
      # discard the return value; the upstream-derived tests under
      # `test/rbs/environment_loader_test.rb` have been adjusted to
      # verify state via the env instead.
      def load(env:)
        if @core_root && libs.none? { |lib| lib.name == "stringio" }
          add(library: "stringio", version: nil)
        end

        Librbs::Native.load_env(
          env,
          core_root&.to_s,
          libs.to_a,
          dirs,
          repository.dirs.map(&:to_s)
        )

        []
      end
    end
  end
end

RBS::EnvironmentLoader.prepend(Librbs::Patches::EnvironmentLoader)
