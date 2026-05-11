# frozen_string_literal: true

require "set"

module Librbs
  module Patches
    module EnvironmentLoader
      # Replace upstream `RBS::EnvironmentLoader#load`. The Ruby side
      # decides *what* directories to walk by invoking `each_dir`, then
      # walks each one through upstream `FileFinder.each_file` (the
      # same primitive `EnvironmentLoader#each_signature` uses) to
      # produce a deduplicated flat list of `.rbs` paths. The Rust
      # bridge receives that list and runs the parallel read + parse +
      # `Environment::add_source` pipeline.
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

        paths = []
        seen = Set.new
        each_dir do |source, dir|
          # `skip_hidden = !source.is_a?(Pathname)` mirrors upstream
          # `each_signature` — `_`-prefixed dirs are hidden for core /
          # library sources, kept for user-supplied paths.
          skip_hidden = !source.is_a?(Pathname)
          RBS::FileFinder.each_file(dir, skip_hidden: skip_hidden) do |path|
            paths << path if seen.add?(path)
          end
        end

        Librbs::Native.load_env(env, paths)

        []
      end
    end
  end
end

RBS::EnvironmentLoader.prepend(Librbs::Patches::EnvironmentLoader)
