# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3k: source-derived APIs (`sources`, `declarations`, `each_rbs_source`,
# `each_ruby_source`) populated via `Librbs::Native.materialize_all`.
RSpec.describe "Librbs::Native.materialize_all sources" do
  it "populates Source::RBS instances with buffer / directives / declarations" do
    Librbs::SpecSupport.with_inline_env("class Foo\nend\n") do |env|
      expect(env.sources.size).to eq(1)
      src = env.sources.first
      expect(src).to be_a(RBS::Source::RBS)
      expect(src.buffer).to be_a(RBS::Buffer)
      expect(src.directives).to eq([])
      expect(src.declarations.size).to eq(1)
      expect(src.declarations.first).to be_a(RBS::AST::Declarations::Class)
    end
  end

  it "preserves object identity between Source#declarations and *_decls entries" do
    Librbs::SpecSupport.with_inline_env("class Foo\nend\n") do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(env.sources.first.declarations.first).to equal(entry.each_decl.first)
    end
  end

  it "exposes each_rbs_source / each_ruby_source through the patch" do
    Librbs::SpecSupport.with_inline_env("class Foo\nend\n") do |env|
      expect(env.each_rbs_source.to_a.size).to eq(env.sources.size)
      expect(env.each_ruby_source.to_a).to eq([])
    end
  end

  it "Environment#declarations returns the flat list of top-level decls" do
    src = <<~RBS
      class Foo
      end
      module Bar
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      expect(env.declarations.size).to eq(2)
      expect(env.declarations.map(&:class)).to eq(
        [RBS::AST::Declarations::Class, RBS::AST::Declarations::Module]
      )
    end
  end

  it "preserves @sources object identity across repeated accesses" do
    Librbs::SpecSupport.with_inline_env("class Foo\nend\n") do |env|
      first = env.sources
      Librbs::Native.materialize_all(env)
      expect(env.sources).to equal(first)
    end
  end
end

RSpec.describe "Librbs::Native.materialize_all directives" do
  it "materializes a single-clause Use directive" do
    src = <<~RBS
      use Foo::Bar

      class Quux
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      directives = env.sources.first.directives
      expect(directives.size).to eq(1)
      use = directives.first
      expect(use).to be_a(RBS::AST::Directives::Use)
      expect(use.clauses.size).to eq(1)
      clause = use.clauses.first
      expect(clause).to be_a(RBS::AST::Directives::Use::SingleClause)
      expect(clause.type_name.to_s).to eq("Foo::Bar")
      expect(clause.new_name).to be_nil
    end
  end

  it "materializes a single-clause Use directive with `as` rename" do
    src = <<~RBS
      use Foo::Bar as Baz

      class Quux
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      clause = env.sources.first.directives.first.clauses.first
      expect(clause).to be_a(RBS::AST::Directives::Use::SingleClause)
      expect(clause.new_name).to eq(:Baz)
    end
  end

  it "materializes a wildcard-clause Use directive" do
    src = <<~RBS
      use Foo::*

      class Quux
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      clause = env.sources.first.directives.first.clauses.first
      expect(clause).to be_a(RBS::AST::Directives::Use::WildcardClause)
      expect(clause.namespace).to be_a(RBS::Namespace)
      expect(clause.namespace.path).to eq([:Foo])
      expect(clause.namespace.absolute?).to be(false)
    end
  end

  it "materializes a ResolveTypeNames directive from the magic comment" do
    src = <<~RBS
      # resolve-type-names: false
      class Foo
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      directives = env.sources.first.directives
      expect(directives.size).to eq(1)
      d = directives.first
      expect(d).to be_a(RBS::AST::Directives::ResolveTypeNames)
      expect(d.value).to be(false)
    end
  end
end
