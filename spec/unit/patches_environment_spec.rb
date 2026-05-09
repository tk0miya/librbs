# frozen_string_literal: true

require "rbs"

# M3i patch-hardening: pure-Ruby `RBS::Environment.new` instances have
# no `@__librbs_handle` and must continue to work through the patched
# accessors. The patches' `instance_variable_defined?` guard is what
# preserves that fallback — assert here that the guard holds and that
# `Librbs::Native.materialize_all` is never reached on this path.
RSpec.describe "Librbs::Patches::Environment fallback" do
  it "does not call into native materialization for a pure-Ruby env" do
    env = RBS::Environment.new
    expect(Librbs::Native).not_to receive(:materialize_all)

    expect(env.class_decls).to eq({})
    expect(env.interface_decls).to eq({})
    expect(env.type_alias_decls).to eq({})
    expect(env.constant_decls).to eq({})
    expect(env.class_alias_decls).to eq({})
    expect(env.global_decls).to eq({})

    expect(env.instance_variable_defined?(:@__librbs_handle)).to be(false)
    expect(env.instance_variable_get(:@__librbs_materialized)).to be_nil
  end

  it "leaves accessors functional after a follow-up call (no caching of a nil result)" do
    env = RBS::Environment.new
    first = env.class_decls
    second = env.class_decls
    expect(second).to equal(first)
  end
end
