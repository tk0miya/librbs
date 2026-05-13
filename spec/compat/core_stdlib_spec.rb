# frozen_string_literal: true

require_relative "../support/canonical_dump"
require_relative "../support/without_librbs"

# Extend the canonical-dump compatibility net beyond pure core to
# core + every stdlib library shipped under `vendor/rbs/stdlib`. The
# loader's `add(library:)` is the single entry point we exercise here;
# anything that materializes for the full stdlib set must round-trip
# byte-for-byte against pure RBS.
RSpec.describe "canonical_dump compatibility (core + stdlib)" do
  STDLIB_LIBRARIES = Dir.children(File.expand_path("../../vendor/rbs/stdlib", __dir__)).sort.freeze

  let(:helper) { File.expand_path("../support/canonical_dump.rb", __dir__) }

  def add_all_stdlib(loader)
    STDLIB_LIBRARIES.each { |lib| loader.add(library: lib, version: nil) }
  end

  def pure_dump_for(setup, resolved:)
    resolve = resolved ? ".resolve_type_names" : ""
    without_librbs(<<~RUBY)
      require "rbs"
      require #{helper.inspect}
      loader = RBS::EnvironmentLoader.new
      #{setup}
      env = RBS::Environment.from_loader(loader)#{resolve}
      print canonical_dump(env)
    RUBY
  end

  it "matches between librbs and pure RBS for unresolved core + stdlib" do
    loader = RBS::EnvironmentLoader.new
    add_all_stdlib(loader)
    librbs_dump = canonical_dump(RBS::Environment.from_loader(loader))

    pure_dump = pure_dump_for(<<~RUBY, resolved: false)
      #{STDLIB_LIBRARIES.inspect}.each { |lib| loader.add(library: lib, version: nil) }
    RUBY

    expect(librbs_dump).to eq(pure_dump)
  end

  it "matches between librbs and pure RBS for resolved core + stdlib" do
    loader = RBS::EnvironmentLoader.new
    add_all_stdlib(loader)
    librbs_dump = canonical_dump(RBS::Environment.from_loader(loader).resolve_type_names)

    pure_dump = pure_dump_for(<<~RUBY, resolved: true)
      #{STDLIB_LIBRARIES.inspect}.each { |lib| loader.add(library: lib, version: nil) }
    RUBY

    expect(librbs_dump).to eq(pure_dump)
  end
end
