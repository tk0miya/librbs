# frozen_string_literal: true

require_relative "../support/inline_env"
require_relative "../support/without_librbs"

# Verifies the M3e plumbing for `RBS::Location` materialization. The
# helpers under `ext/librbs/src/materialize/location.rs` ought to
# produce a `Location` whose start_line / start_column match what
# pure RBS would compute for the same range. The multi-byte case is
# the regression guard against any byte-vs-character offset mix-up
# sneaking in during M3f-h.
RSpec.describe "Librbs::Native materialize/location plumbing" do
  it "produces start_line/start_column matching pure RBS for ASCII source" do
    Librbs::SpecSupport.with_inline_env("class Foo\nend\n") do |env|
      loc = Librbs::Native._materialize_first_decl_location(env)
      expect(loc).to be_a(RBS::Location)
      expect(loc.start_line).to eq(1)
      expect(loc.start_column).to eq(0)
      expect(loc.source).to eq("class Foo\nend")
    end
  end

  it "produces start_line/start_column matching pure RBS for the multi-byte fixture" do
    fixture = File.expand_path("../fixtures/multibyte.rbs", __dir__)
    Librbs::SpecSupport.with_fixture_env(fixture) do |env|
      loc = Librbs::Native._materialize_first_decl_location(env)
      expect(loc).to be_a(RBS::Location)

      # The fixture has multi-byte comment lines before the first
      # declaration. The class starts on line 4, column 0; mismatching
      # lines or columns indicates the helper is doing byte arithmetic
      # somewhere it shouldn't.
      expect(loc.start_line).to eq(5)
      expect(loc.start_column).to eq(0)
      expect(loc.source).to start_with("class Foo")

      # Cross-check against a pure-RBS subprocess: pure RBS reading the
      # same fixture must agree on (start_line, start_column) for the
      # `class Foo` declaration. This is the regression contract from
      # the M3e acceptance list.
      pure = without_librbs(<<~RUBY)
        require "rbs"
        loader = RBS::EnvironmentLoader.new(core_root: nil)
        loader.add(path: Pathname(#{File.dirname(fixture).inspect}))
        env = RBS::Environment.from_loader(loader)
        decl = env.class_decls.values.first.primary_decl
        print "\#{decl.location.start_line}:\#{decl.location.start_column}"
      RUBY
      expect(pure).to eq("#{loc.start_line}:#{loc.start_column}")
    end
  end

  it "shares one RBS::Buffer across all decls of the same source (identity)" do
    # `MaterializeCtx::buffer` caches the `RBS::Buffer` per source
    # index. `_materialize_all_decl_locations` runs every decl through
    # ONE ctx, so all returned Locations must share the very same
    # Buffer object (upstream RBS uses Buffer identity in some equality
    # checks; value equivalence isn't enough).
    Librbs::SpecSupport.with_inline_env("class A end\nclass B end\nclass C end\n") do |env|
      locs = Librbs::Native._materialize_all_decl_locations(env)
      expect(locs.size).to eq(3)
      buffers = locs.map(&:buffer)
      expect(buffers[0]).to be(buffers[1])
      expect(buffers[1]).to be(buffers[2])
    end
  end
end
