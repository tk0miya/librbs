# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3k Y1: directive materialiser. Verifies that
# `RBS::Source::RBS#directives` exposes `Use` (with single / wildcard
# clauses) and `ResolveTypeNames` instances mirroring upstream's
# parser + magic-comment scanner.
RSpec.describe "Librbs::Native materialize/directive parity" do
  it "materialises a `# use Foo::Bar` directive as a Use::SingleClause" do
    src = <<~RBS
      use Foo::Bar

      class Top
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      directives = env.sources.first.directives
      use = directives.find { |d| d.is_a?(RBS::AST::Directives::Use) }
      expect(use).not_to be_nil
      expect(use.clauses.size).to eq(1)
      clause = use.clauses.first
      expect(clause).to be_a(RBS::AST::Directives::Use::SingleClause)
      expect(clause.type_name.to_s).to eq("Foo::Bar")
      expect(clause.new_name).to be_nil
    end
  end

  it "materialises `# use Foo::Bar as FB` with a renamed alias" do
    src = <<~RBS
      use Foo::Bar as FB

      class Top
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      use = env.sources.first.directives.find { |d| d.is_a?(RBS::AST::Directives::Use) }
      clause = use.clauses.first
      expect(clause).to be_a(RBS::AST::Directives::Use::SingleClause)
      expect(clause.new_name).to eq(:FB)
    end
  end

  it "materialises `# use Foo::*` as a Use::WildcardClause" do
    src = <<~RBS
      use Foo::*

      class Top
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      use = env.sources.first.directives.find { |d| d.is_a?(RBS::AST::Directives::Use) }
      clause = use.clauses.first
      expect(clause).to be_a(RBS::AST::Directives::Use::WildcardClause)
      expect(clause.namespace.to_s).to eq("Foo::")
    end
  end

  it "materialises `# resolve-type-names: false` as a ResolveTypeNames directive" do
    src = <<~RBS
      # resolve-type-names: false

      class Top
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      directives = env.sources.first.directives
      magic = directives.find { |d| d.is_a?(RBS::AST::Directives::ResolveTypeNames) }
      expect(magic).not_to be_nil
      expect(magic.value).to be(false)
    end
  end

  it "materialises `# resolve-type-names: true` as a ResolveTypeNames directive" do
    src = <<~RBS
      # resolve-type-names: true

      class Top
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      magic = env.sources.first.directives.find { |d| d.is_a?(RBS::AST::Directives::ResolveTypeNames) }
      expect(magic).not_to be_nil
      expect(magic.value).to be(true)
    end
  end

  it "places the magic comment first when both kinds of directive coexist" do
    src = <<~RBS
      # resolve-type-names: false
      use Foo::Bar

      class Top
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      directives = env.sources.first.directives
      expect(directives.size).to eq(2)
      expect(directives[0]).to be_a(RBS::AST::Directives::ResolveTypeNames)
      expect(directives[1]).to be_a(RBS::AST::Directives::Use)
    end
  end
end
