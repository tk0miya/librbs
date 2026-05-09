# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3h: end-to-end materialization of `RBS::Environment::*Entry` and
# the six `*_decls` hashes via `Librbs::Native.materialize_all`. Most
# fine-grained coverage (per-type, per-member byte-equivalence) lives
# in `spec/compat/canonical_dump_core_spec.rb`; this file pins the
# materialization invariants the canonical dump alone wouldn't catch
# (re-entrancy, accessor-triggered build, pure-Ruby env fallback).
RSpec.describe "Librbs::Native.materialize_all" do
  it "populates class_decls with ClassEntry / ModuleEntry whose decls are RBS::AST::Declarations::*" do
    src = <<~RBS
      class Foo
        def bar: () -> Integer
      end
      module Mixin
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      class_decls = env.class_decls
      foo = class_decls[RBS::TypeName.parse("::Foo")]
      mixin = class_decls[RBS::TypeName.parse("::Mixin")]

      expect(foo).to be_a(RBS::Environment::ClassEntry)
      expect(mixin).to be_a(RBS::Environment::ModuleEntry)

      foo_decl = foo.each_decl.first
      expect(foo_decl).to be_a(RBS::AST::Declarations::Class)
      expect(foo_decl.name).to eq(RBS::TypeName.parse("::Foo"))
      expect(foo_decl.members.first).to be_a(RBS::AST::Members::MethodDefinition)
    end
  end

  it "is re-entrant: calling materialize_all twice is a no-op and accessors return the same Hash" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      first = env.class_decls
      Librbs::Native.materialize_all(env)
      second = env.class_decls
      expect(second).to equal(first)
    end
  end

  it "auto-triggers materialization on first accessor and caches the result" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      expect(env.instance_variable_get(:@__librbs_materialized)).to be_nil
      _ = env.class_decls
      expect(env.instance_variable_get(:@__librbs_materialized)).to be(true)
    end
  end

  it "leaves a pure-Ruby RBS::Environment.new alone (super() returns the empty default Hashes)" do
    env = RBS::Environment.new
    expect(env.class_decls).to eq({})
    expect(env.interface_decls).to eq({})
    expect(env.global_decls).to eq({})
    # The patch's `instance_variable_defined?` guard prevents an
    # ensure_materialized → Native.materialize_all attempt on a
    # handle-less env.
    expect(env.instance_variable_get(:@__librbs_materialized)).to be_nil
  end

  it "populates global_decls keyed by Symbol" do
    Librbs::SpecSupport.with_inline_env("$logger: Integer\n") do |env|
      entry = env.global_decls[:$logger]
      expect(entry).to be_a(RBS::Environment::GlobalEntry)
      expect(entry.decl).to be_a(RBS::AST::Declarations::Global)
      expect(entry.decl.name).to eq(:$logger)
    end
  end

  it "populates type_alias_decls and constant_decls" do
    src = <<~RBS
      type my_alias = Integer | String
      Pi: Float
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      alias_entry = env.type_alias_decls[RBS::TypeName.parse("::my_alias")]
      const_entry = env.constant_decls[RBS::TypeName.parse("::Pi")]
      expect(alias_entry).to be_a(RBS::Environment::TypeAliasEntry)
      expect(alias_entry.decl).to be_a(RBS::AST::Declarations::TypeAlias)
      expect(const_entry).to be_a(RBS::Environment::ConstantEntry)
      expect(const_entry.decl).to be_a(RBS::AST::Declarations::Constant)
    end
  end

  it "records nested decls under their fully-qualified name" do
    src = <<~RBS
      module Outer
        class Inner
        end
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      inner = env.class_decls[RBS::TypeName.parse("::Outer::Inner")]
      expect(inner).to be_a(RBS::Environment::ClassEntry)
      expect(inner.each_decl.first).to be_a(RBS::AST::Declarations::Class)
    end
  end
end
