# frozen_string_literal: true

require "tmpdir"
require "rbs"

module Librbs
  module SpecSupport
    # Build an `RBS::Environment` from an in-memory RBS source string.
    # The source is written to a temporary directory which the loader
    # discovers via `add(path:)`. Equivalent fixtures on disk would
    # work too, but inline strings keep the spec scenarios local to
    # the example. Yields the env and cleans up the tmpdir afterwards.
    def self.with_inline_env(content, &block)
      Dir.mktmpdir do |dir|
        File.write(File.join(dir, "test.rbs"), content)
        loader = RBS::EnvironmentLoader.new(core_root: nil)
        loader.add(path: Pathname(dir))
        env = RBS::Environment.from_loader(loader)
        block.call(env)
      end
    end

    # Build an env from an existing on-disk RBS file. The file is
    # copied into a temporary directory so the loader doesn't pick up
    # sibling fixtures, then discovered via `add(path:)`.
    def self.with_fixture_env(path, &block)
      Dir.mktmpdir do |dir|
        File.write(File.join(dir, File.basename(path)), File.read(path))
        loader = RBS::EnvironmentLoader.new(core_root: nil)
        loader.add(path: Pathname(dir))
        block.call(RBS::Environment.from_loader(loader))
      end
    end
  end
end
