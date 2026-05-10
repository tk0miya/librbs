# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"
require_relative "../support/without_librbs"

# M3e plumbing for `RBS::Location` materialization. Writer-based
# golden specs (M3j) intentionally run in non-preserve mode and so
# never read `loc.source`; they cannot catch a regression in the
# byte-vs-character offsets, an off-by-one in a child range, or a
# `RBS::Buffer` identity slip across decls of the same source. This
# file owns those invariants.
#
# This is the M3j-era replacement for the harness-coupled
# `_materialize_first_decl_location` /
# `_materialize_all_decl_locations` test entries that M3h removed:
# instead of reaching into a private singleton method, the spec
# observes locations through the public `RBS::Environment` accessor
# path (`class_decls -> ClassEntry#each_decl -> decl.location`).
RSpec.describe "Librbs::Native materialize/location plumbing" do
  it "produces start_line/start_column for an ASCII source" do
    Librbs::SpecSupport.with_inline_env("class Foo\nend\n") do |env|
      decl = env.class_decls[RBS::TypeName.parse("::Foo")].each_decl.first
      loc = decl.location
      expect(loc).to be_a(RBS::Location)
      expect(loc.start_line).to eq(1)
      expect(loc.start_column).to eq(0)
      expect(loc.source).to eq("class Foo\nend")
    end
  end

  it "uses character (not byte) offsets for a multi-byte source" do
    fixture = File.expand_path("../fixtures/multibyte.rbs", __dir__)
    Librbs::SpecSupport.with_fixture_env(fixture) do |env|
      decl = env.class_decls[RBS::TypeName.parse("::Foo")].each_decl.first
      loc = decl.location

      # The fixture has 4 multi-byte comment lines before the first
      # declaration. The class starts on line 5, column 0; mismatching
      # line or column indicates the helper is doing byte arithmetic
      # somewhere it shouldn't.
      expect(loc.start_line).to eq(5)
      expect(loc.start_column).to eq(0)
      expect(loc.source).to start_with("class Foo")

      # Cross-check against a pure-RBS subprocess: pure RBS reading the
      # same fixture must agree on (start_line, start_column) for the
      # `class Foo` declaration. The regression contract from the
      # original M3e acceptance.
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

  it "shares one RBS::Buffer across every decl materialized from the same source" do
    # `MaterializeCtx::buffer` caches the `RBS::Buffer` per source
    # index so all decls coming from one source share a Buffer
    # identity. Upstream RBS uses Buffer identity in some equality
    # checks; value equivalence isn't enough.
    src = <<~RBS
      class A
      end
      class B
      end
      class C
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      decls = %w[::A ::B ::C].map { |n| env.class_decls[RBS::TypeName.parse(n)].each_decl.first }
      buffers = decls.map { |d| d.location.buffer }
      expect(buffers[0]).to be(buffers[1])
      expect(buffers[1]).to be(buffers[2])
    end
  end

  it "shares the source's Buffer with nested decls and members" do
    # Nested decls and member-level locations (e.g. method
    # definitions) materialize through the same MaterializeCtx, so
    # they must share the parent's Buffer. A Buffer slip here would
    # break upstream's `loc.buffer == other_loc.buffer` checks at
    # the member granularity.
    src = <<~RBS
      module Outer
        class Inner
          def m: () -> void
        end
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      outer = env.class_decls[RBS::TypeName.parse("::Outer")].each_decl.first
      inner = env.class_decls[RBS::TypeName.parse("::Outer::Inner")].each_decl.first
      method = inner.members.first
      expect(outer.location.buffer).to be(inner.location.buffer)
      expect(inner.location.buffer).to be(method.location.buffer)
    end
  end
end
