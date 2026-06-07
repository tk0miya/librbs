# frozen_string_literal: true

require "rbs"

# A librbs-built env keeps its state in the Rust handle until the first
# materializing access. Methods that read the bare ivars before that point
# would otherwise observe empty Hashes. These specs lock in the trigger
# coverage for `inspect` and `dup`/`clone` (`initialize_copy`), each
# exercised *before* any other access so the pre-materialize path is the
# one under test.
RSpec.describe "Librbs::Patches::Environment materialize triggers" do
  def fresh_env
    RBS::Environment.from_loader(RBS::EnvironmentLoader.new)
  end

  describe "#inspect" do
    it "reports real sizes when called before any other access" do
      env = fresh_env
      # Upstream `inspect` reads `@class_decls.size` & co. directly; without
      # the trigger this would print `(0 items)` for a populated env.
      expect(env.inspect).not_to include("@class_decls=(0 items)")
    end

    it "agrees with the materialized accessors" do
      env = fresh_env
      size = env.class_decls.size
      expect(size).to be > 0
      expect(env.inspect).to include("@class_decls=(#{size} items)")
    end

    it "still reports (0 items) for an empty pure-Ruby env" do
      expect(RBS::Environment.new.inspect).to include("@class_decls=(0 items)")
    end
  end

  describe "#initialize_copy (dup/clone)" do
    it "dup taken before any access sees the fully materialized decls" do
      env = fresh_env
      copy = env.dup
      expect(copy.class_decls).to eq(env.class_decls)
      expect(copy.class_decls.size).to be > 0
    end

    it "severs the Rust handle so the copy is a self-contained Ruby env" do
      env = fresh_env
      copy = env.dup
      expect(copy.instance_variable_defined?(:@__librbs_handle)).to be(false)
      # A second read must not reach back into native materialization.
      expect(Librbs::Native).not_to receive(:materialize_all)
      expect(copy.class_decls.size).to be > 0
    end

    it "dup of an already-materialized env still matches" do
      env = fresh_env
      env.class_decls # force materialization first
      copy = env.dup
      expect(copy.class_decls).to eq(env.class_decls)
    end

    it "leaves pure-Ruby env dup working" do
      env = RBS::Environment.new
      expect { env.dup.class_decls }.not_to raise_error
      expect(env.dup.class_decls).to eq({})
    end
  end
end
