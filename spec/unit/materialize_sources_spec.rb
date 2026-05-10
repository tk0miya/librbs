# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3k Y2: `Environment#sources` / `#declarations` / `each_*_source`
# parity with upstream. After materialisation, each Rust source must
# surface as a `Source::RBS` whose decls share Ruby object identity
# with the matching `*_decls` entry — at every nesting level.
RSpec.describe "Librbs::Native materialize/source parity" do
  it "exposes one Source::RBS per source via #sources" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      expect(env.sources.size).to eq(1)
      expect(env.sources.first).to be_a(RBS::Source::RBS)
    end
  end

  it "preserves object identity between source.declarations and class_decls entries" do
    src = <<~RBS
      class Foo
        def bar: () -> Integer
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      source_decl = env.sources.first.declarations.first
      entry_decl = env.class_decls[RBS::TypeName.parse("::Foo")].each_decl.first
      expect(source_decl).to equal(entry_decl)
    end
  end

  it "preserves object identity for nested decls reachable via members" do
    src = <<~RBS
      module Outer
        class Inner
        end
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      outer_decl = env.sources.first.declarations.first
      inner_via_members = outer_decl.members.first
      inner_via_class_decls =
        env.class_decls[RBS::TypeName.parse("::Outer::Inner")].each_decl.first
      expect(inner_via_members).to equal(inner_via_class_decls)
    end
  end

  it "exposes #declarations as the flat_map of source declarations" do
    src = <<~RBS
      class A
      end
      class B
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      decls = env.declarations
      expect(decls.size).to eq(2)
      expect(decls.first).to equal(env.sources.first.declarations.first)
    end
  end

  it "yields rbs sources via each_rbs_source and never via each_ruby_source" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      expect(env.each_rbs_source.to_a.size).to eq(env.sources.size)
      expect(env.each_ruby_source.to_a).to eq([])
    end
  end

  it "preserves @sources object identity across repeat accessor calls" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      first = env.sources
      Librbs::Native.materialize_all(env)
      expect(env.sources).to equal(first)
    end
  end

  it "matches the pure-RBS declaration count for a curated fixture" do
    src = <<~RBS
      class Foo
        def bar: () -> Integer

        class Inner
        end
      end

      module M
      end

      type t = Integer

      $g: String

      Pi: Float
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      # Top-level: class Foo, module M, type t, $g, Pi → 5 declarations
      # (nested Inner does not appear at top level).
      expect(env.declarations.size).to eq(5)
    end
  end
end
