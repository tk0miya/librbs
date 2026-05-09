# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3k: `RBS::Environment#sources` / `#declarations` /
# `#each_rbs_source` / `#each_ruby_source` parity with upstream.
RSpec.describe "Librbs::Native.materialize_all (sources)" do
  it "populates sources with RBS::Source::RBS instances after first accessor" do
    src = <<~RBS
      class Foo
      end
      module Mixin
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      _ = env.class_decls
      expect(env.sources).not_to be_empty
      expect(env.sources).to all(be_a(RBS::Source::RBS))
      expect(env.sources.first.declarations).not_to be_empty
      expect(env.sources.first.directives).to eq([])
    end
  end

  it "auto-materializes on env.sources access" do
    Librbs::SpecSupport.with_inline_env("class Foo end\n") do |env|
      expect(env.instance_variable_get(:@__librbs_materialized)).to be_nil
      _ = env.sources
      expect(env.instance_variable_get(:@__librbs_materialized)).to be(true)
    end
  end

  it "auto-materializes on env.declarations access" do
    Librbs::SpecSupport.with_inline_env("class Foo end\n") do |env|
      expect(env.instance_variable_get(:@__librbs_materialized)).to be_nil
      decls = env.declarations
      expect(env.instance_variable_get(:@__librbs_materialized)).to be(true)
      expect(decls).not_to be_empty
    end
  end

  it "env.declarations.size matches the number of top-level decls" do
    src = <<~RBS
      class Foo
        class Inner end
      end
      module Bar end
      type baz = Integer
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      expect(env.declarations.size).to eq(3)
    end
  end

  it "preserves Ruby object identity between source.declarations and *_decls entries" do
    Librbs::SpecSupport.with_inline_env("class Foo end\n") do |env|
      class_decls = env.class_decls
      entry_decl = class_decls[RBS::TypeName.parse("::Foo")].each_decl.first
      source_decl = env.sources.first.declarations.first
      expect(source_decl).to equal(entry_decl)
    end
  end

  it "each_rbs_source yields all RBS sources and each_ruby_source yields nothing" do
    Librbs::SpecSupport.with_inline_env("class Foo end\n") do |env|
      expect(env.each_rbs_source.to_a.size).to eq(env.sources.size)
      expect(env.each_ruby_source.to_a).to be_empty
    end
  end

  it "is re-entrant: env.sources returns the same Array on repeat access" do
    Librbs::SpecSupport.with_inline_env("class Foo end\n") do |env|
      first = env.sources
      second = env.sources
      expect(second).to equal(first)
    end
  end

  it "leaves a pure-Ruby RBS::Environment.new alone for sources/declarations" do
    env = RBS::Environment.new
    expect(env.sources).to eq([])
    expect(env.declarations).to eq([])
    expect(env.each_rbs_source.to_a).to eq([])
    expect(env.each_ruby_source.to_a).to eq([])
    expect(env.instance_variable_get(:@__librbs_materialized)).to be_nil
  end

  it "preserves the intra-env identity invariant after resolve_type_names" do
    Librbs::SpecSupport.with_inline_env("class Foo end\n") do |env|
      resolved = env.resolve_type_names
      foo = resolved.class_decls[RBS::TypeName.parse("::Foo")]
      entry_decl = foo.each_decl.first
      source_decl = resolved.sources.first.declarations.first
      expect(source_decl).to equal(entry_decl)
    end
  end

  it "resolve_type_names produces a fresh sources array distinct from the source env" do
    Librbs::SpecSupport.with_inline_env("class Foo end\n") do |env|
      original_sources = env.sources
      resolved = env.resolve_type_names
      expect(resolved.sources).not_to equal(original_sources)
    end
  end
end
