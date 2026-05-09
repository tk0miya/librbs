# frozen_string_literal: true

require "set"

require_relative "../support/canonical_dump"
require_relative "../support/without_librbs"

# Compatibility check: the Ruby canonical dumper applied to a librbs-built
# `RBS::Environment` must agree with the same dumper applied to a pure-RBS
# environment built in a fresh subprocess. End-to-end regression net for
# the M3h materialization cut-over — covers Locations, TypeNames,
# member shapes, and decl-level type_params on a real stdlib subset.
RSpec.describe "canonical_dump compatibility (core)" do
  it "matches between librbs and pure RBS for unresolved core" do
    loader = RBS::EnvironmentLoader.new
    env = RBS::Environment.from_loader(loader)
    librbs_dump = canonical_dump(env)

    helper = File.expand_path("../support/canonical_dump.rb", __dir__)
    pure_dump = without_librbs(<<~RUBY)
      require "rbs"
      require #{helper.inspect}
      env = RBS::Environment.from_loader(RBS::EnvironmentLoader.new)
      print canonical_dump(env)
    RUBY

    expect(librbs_dump).to eq(pure_dump)
  end

  it "exposes a non-empty pure-RBS dump for the unresolved core (sanity)" do
    helper = File.expand_path("../support/canonical_dump.rb", __dir__)
    pure_dump = without_librbs(<<~RUBY)
      require "rbs"
      require #{helper.inspect}
      env = RBS::Environment.from_loader(RBS::EnvironmentLoader.new)
      print canonical_dump(env)
    RUBY

    expect(pure_dump).to start_with("== class_decls ==\n")
    expect(pure_dump).to include("class ::Object\n")
  end

  # Resolved variant — same shape as the unresolved spec above, with
  # `resolve_type_names` applied to both sides.
  it "matches between librbs and pure RBS for resolved core" do
    loader = RBS::EnvironmentLoader.new
    env = RBS::Environment.from_loader(loader).resolve_type_names
    librbs_dump = canonical_dump(env)

    helper = File.expand_path("../support/canonical_dump.rb", __dir__)
    pure_dump = without_librbs(<<~RUBY)
      require "rbs"
      require #{helper.inspect}
      env = RBS::Environment.from_loader(RBS::EnvironmentLoader.new).resolve_type_names
      print canonical_dump(env)
    RUBY

    expect(librbs_dump).to eq(pure_dump)
  end

  it "returns a fresh RBS::Environment with shared handle and a resolution side-table" do
    # Smoke test for the M3d native bridge that does not depend on
    # materialization. Verifies the documented post-conditions:
    # `@__librbs_handle` is shared (object identity) and
    # `@__librbs_resolution` is populated.
    loader = RBS::EnvironmentLoader.new
    src = RBS::Environment.from_loader(loader)
    dst = src.resolve_type_names

    expect(dst).to be_a(RBS::Environment)
    expect(dst).not_to equal(src)
    expect(dst.instance_variable_get(:@__librbs_handle))
      .to equal(src.instance_variable_get(:@__librbs_handle))
    expect(dst.instance_variable_get(:@__librbs_resolution)).not_to be_nil
  end

  it "honors only: by restricting which decls drive resolution" do
    # `only:` is a Set[TypeName]; the patch turns it into an Array
    # before crossing the magnus boundary. The native side must accept
    # both an empty set (resolve nothing) and a filled set without
    # raising.
    loader = RBS::EnvironmentLoader.new
    env = RBS::Environment.from_loader(loader)

    empty = env.resolve_type_names(only: Set.new)
    expect(empty.instance_variable_get(:@__librbs_resolution)).not_to be_nil

    one = env.resolve_type_names(only: Set[RBS::TypeName.parse("::Object")])
    expect(one.instance_variable_get(:@__librbs_resolution)).not_to be_nil
  end
end
