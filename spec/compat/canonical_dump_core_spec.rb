# frozen_string_literal: true

require_relative "../support/canonical_dump"
require_relative "../support/without_librbs"

# Compatibility check: the Ruby canonical dumper applied to a librbs-built
# `RBS::Environment` must agree with the same dumper applied to a pure-RBS
# environment built in a fresh subprocess.
#
# **Currently pending.** M3c builds the Rust `Environment` and stashes it
# in `@__librbs_handle`, but materialization back into Ruby
# `@class_decls` etc. is M3e. Until then `canonical_dump(librbs_env)`
# walks empty hashes and cannot agree with the pure-RBS dump. The
# expectation is wired up here so that flipping `pending` off is the
# only change needed once M3e lands.
RSpec.describe "canonical_dump compatibility (core)" do
  it "matches between librbs and pure RBS for unresolved core" do
    pending "Ruby-side canonical_dump requires M3e materialization to populate @class_decls"

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
end
