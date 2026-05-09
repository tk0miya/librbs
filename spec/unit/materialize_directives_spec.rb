# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3k: per-source directives — `Use` (single + wildcard clauses) and
# `ResolveTypeNames` (magic comment).
RSpec.describe "Librbs::Native.materialize_all (directives)" do
  it "produces Use directive with SingleClause and WildcardClause for `use Foo::Bar` / `use Foo::*`" do
    src = <<~RBS
      use Foo::Bar
      use Foo::*

      class Baz end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      sources = env.sources
      expect(sources.size).to eq(1)
      directives = sources.first.directives

      uses = directives.select { |d| d.is_a?(RBS::AST::Directives::Use) }
      expect(uses.size).to eq(2)

      first = uses[0].clauses.first
      expect(first).to be_a(RBS::AST::Directives::Use::SingleClause)
      expect(first.type_name.name).to eq(:Bar)
      expect(first.type_name.namespace.path).to eq([:Foo])
      expect(first.type_name.namespace.absolute?).to be(false)
      expect(first.new_name).to be_nil

      second = uses[1].clauses.first
      expect(second).to be_a(RBS::AST::Directives::Use::WildcardClause)
      expect(second.namespace.path).to eq([:Foo])
      expect(second.namespace.absolute?).to be(false)
    end
  end

  it "captures `as` rename in SingleClause#new_name" do
    src = <<~RBS
      use ::Foo::Bar as Baz

      class Quux end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      use = env.sources.first.directives.first
      expect(use).to be_a(RBS::AST::Directives::Use)
      clause = use.clauses.first
      expect(clause).to be_a(RBS::AST::Directives::Use::SingleClause)
      expect(clause.new_name).to eq(:Baz)
      expect(clause.type_name.namespace.absolute?).to be(true)
    end
  end

  it "produces a ResolveTypeNames directive when the magic comment is present" do
    src = <<~RBS
      # resolve-type-names: false

      class Foo end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      first = env.sources.first.directives.first
      expect(first).to be_a(RBS::AST::Directives::ResolveTypeNames)
      expect(first.value).to be(false)
    end
  end

  it "honors a `true` value for the magic comment" do
    src = <<~RBS
      # resolve-type-names: true

      class Foo end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      first = env.sources.first.directives.first
      expect(first).to be_a(RBS::AST::Directives::ResolveTypeNames)
      expect(first.value).to be(true)
    end
  end
end
