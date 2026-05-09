# frozen_string_literal: true

require_relative "../support/canonical_dump"
require_relative "../support/without_librbs"

# M3i: per-gem canonical-dump compatibility for the major gems we
# expect downstream tools to load on top of core. Each gem runs
# independently against pure RBS in a fresh subprocess; if the
# library isn't installed in this RBS distribution the example is
# marked `pending` rather than failing the suite.
RSpec.describe "canonical_dump compatibility (gems)" do
  GEM_LIBRARIES = %w[json set bigdecimal csv pathname tempfile time uri].freeze

  let(:helper) { File.expand_path("../support/canonical_dump.rb", __dir__) }

  # `loader.add(library:)` only records the request — sig discovery
  # happens later during `each_signature` / `from_loader`. Probe the
  # latter through upstream's iterator so a missing gem surfaces as
  # `UnknownLibraryError` here, before the librbs path runs.
  def gem_available?(name)
    loader = RBS::EnvironmentLoader.new(core_root: nil)
    loader.add(library: name, version: nil)
    loader.each_signature {} # raise UnknownLibraryError if sigs missing
    true
  rescue RBS::EnvironmentLoader::UnknownLibraryError
    false
  end

  GEM_LIBRARIES.each do |gem|
    context "with #{gem}" do
      before do
        skip "RBS sigs for `#{gem}` are not installed" unless gem_available?(gem)
      end

      it "matches between librbs and pure RBS (unresolved)" do
        loader = RBS::EnvironmentLoader.new
        loader.add(library: gem, version: nil)
        librbs_dump = canonical_dump(RBS::Environment.from_loader(loader))

        pure_dump = without_librbs(<<~RUBY)
          require "rbs"
          require #{helper.inspect}
          loader = RBS::EnvironmentLoader.new
          loader.add(library: #{gem.inspect}, version: nil)
          env = RBS::Environment.from_loader(loader)
          print canonical_dump(env)
        RUBY

        expect(librbs_dump).to eq(pure_dump)
      end

      it "matches between librbs and pure RBS (resolved)" do
        loader = RBS::EnvironmentLoader.new
        loader.add(library: gem, version: nil)
        librbs_dump = canonical_dump(RBS::Environment.from_loader(loader).resolve_type_names)

        pure_dump = without_librbs(<<~RUBY)
          require "rbs"
          require #{helper.inspect}
          loader = RBS::EnvironmentLoader.new
          loader.add(library: #{gem.inspect}, version: nil)
          env = RBS::Environment.from_loader(loader).resolve_type_names
          print canonical_dump(env)
        RUBY

        expect(librbs_dump).to eq(pure_dump)
      end
    end
  end
end
