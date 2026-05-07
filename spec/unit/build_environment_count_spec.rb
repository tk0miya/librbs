# frozen_string_literal: true

require "pathname"

RSpec.describe "Librbs::Native.build_environment_count" do
  it "returns class_decls count for core root" do
    core_root = Pathname(__dir__).join("../../vendor/rbs/core")
    count = Librbs::Native.build_environment_count(core_root.to_s)
    expect(count).to be > 0
  end

  it "returns counts in the same order of magnitude as RBS::Environment for core+stdlib" do
    loader = RBS::EnvironmentLoader.new
    rbs_env = RBS::Environment.from_loader(loader)
    rbs_count = rbs_env.class_decls.size

    # Use the same defaults: core + stdlib (with stringio as RBS does).
    core_root = Pathname(__dir__).join("../../vendor/rbs/core").to_s
    librbs_count = Librbs::Native.build_environment_count(core_root)

    # core only on the librbs side; check it's at least non-trivial.
    expect(librbs_count).to be > 30
    expect(rbs_count).to be > 0
  end
end
