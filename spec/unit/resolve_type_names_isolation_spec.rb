# frozen_string_literal: true

require "rbs"

require_relative "../support/canonical_dump"
require_relative "../support/inline_env"

# Regression for the "`resolve_type_names` mutates the source env's
# shared core state" followup. Upstream's
# `RBS::Environment#resolve_type_names` is a pure function on `self`:
# it returns a fresh env without disturbing the receiver. Before the
# fix, the native path mutated the receiver's `Arc<Environment>` in
# place and stitched the same `WrappedEnvironment` onto the result,
# so `src.@__librbs_handle.equal?(dst.@__librbs_handle)` was `true`
# and any resolver-driven state writes leaked back into `src`.
#
# The fix moves resolution onto a freshly allocated core env via
# `Environment::fork_for_resolution`. The assertions here pin both
# halves: the Ruby-visible handle identity must diverge, and the
# canonical dump of `src` must not change across the call.
RSpec.describe "RBS::Environment#resolve_type_names isolation" do
  let(:rbs_source) do
    <<~RBS
      module M
        class A end
        class B < A end
      end

      class ::Top
        include M::A
      end
    RBS
  end

  it "allocates a fresh @__librbs_handle for the returned env" do
    Librbs::SpecSupport.with_inline_env(rbs_source) do |src|
      src_handle = src.instance_variable_get(:@__librbs_handle)
      expect(src_handle).not_to be_nil

      dst = src.resolve_type_names
      dst_handle = dst.instance_variable_get(:@__librbs_handle)

      # The new contract: the resolved env wraps a *new*
      # `Arc<Environment>`, not the source env's. Object identity is
      # the assertion that survives even if the wrapped Arc's
      # contents end up byte-identical.
      expect(dst_handle).not_to equal(src_handle)
    end
  end

  it "leaves the source env's canonical dump unchanged" do
    Librbs::SpecSupport.with_inline_env(rbs_source) do |src|
      before = canonical_dump(src)
      _ = src.resolve_type_names
      after = canonical_dump(src)

      expect(after).to eq(before)
    end
  end
end
