# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"
require_relative "../support/writer_oracle"

# End-to-end materialization of `RBS::Environment::*Entry` and the
# six `*_decls` hashes via `Librbs::Native.materialize_all`. Most
# fine-grained coverage (per-type, per-member byte-equivalence) lives
# in `spec/compat/canonical_dump_core_spec.rb`; this file pins the
# materialization invariants the canonical dump alone wouldn't catch
# (per-decl printed shape, re-entrancy, accessor-triggered build,
# pure-Ruby env fallback).
#
# The entry-shape examples below assert their post-conditions via
# `RBS::Writer` golden strings (see `spec/support/writer_oracle.rb`)
# so each `write_decl` branch — Class, Module, Interface, TypeAlias,
# Constant, Global, ClassAlias, ModuleAlias — is exercised against
# user-visible RBS syntax rather than `is_a?` chains. Open-class
# iteration order and nested-decl naming each get one example too.
RSpec.describe "Librbs::Native.materialize_all" do
  WriterOracle = Librbs::SpecSupport::WriterOracle

  it "materializes a Class declaration with members" do
    src = <<~RBS
      class Foo
        def bar: () -> Integer
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(entry).to be_a(RBS::Environment::ClassEntry)
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Foo
          def bar: () -> Integer
        end
      RBS
    end
  end

  it "materializes a Module declaration" do
    Librbs::SpecSupport.with_inline_env("module Mixin\nend\n") do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Mixin")]
      expect(entry).to be_a(RBS::Environment::ModuleEntry)
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        module Mixin
        end
      RBS
    end
  end

  it "materializes an Interface declaration" do
    src = <<~RBS
      interface _Each
        def each: () -> void
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.interface_decls[RBS::TypeName.parse("::_Each")]
      expect(entry).to be_a(RBS::Environment::InterfaceEntry)
      expect(WriterOracle.write(entry.decl)).to eq(<<~RBS)
        interface _Each
          def each: () -> void
        end
      RBS
    end
  end

  it "materializes a TypeAlias declaration" do
    Librbs::SpecSupport.with_inline_env("type my_alias = Integer | String\n") do |env|
      entry = env.type_alias_decls[RBS::TypeName.parse("::my_alias")]
      expect(entry).to be_a(RBS::Environment::TypeAliasEntry)
      expect(WriterOracle.write(entry.decl)).to eq("type my_alias = Integer | String\n")
    end
  end

  it "materializes a Constant declaration" do
    Librbs::SpecSupport.with_inline_env("Pi: Float\n") do |env|
      entry = env.constant_decls[RBS::TypeName.parse("::Pi")]
      expect(entry).to be_a(RBS::Environment::ConstantEntry)
      expect(WriterOracle.write(entry.decl)).to eq("Pi: Float\n")
    end
  end

  it "materializes a Global declaration keyed by Symbol" do
    Librbs::SpecSupport.with_inline_env("$logger: String\n") do |env|
      entry = env.global_decls[:$logger]
      expect(entry).to be_a(RBS::Environment::GlobalEntry)
      expect(WriterOracle.write(entry.decl)).to eq("$logger: String\n")
    end
  end

  it "materializes a ClassAlias declaration" do
    src = <<~RBS
      class Foo
      end
      class CAlias = Foo
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_alias_decls[RBS::TypeName.parse("::CAlias")]
      expect(entry).to be_a(RBS::Environment::ClassAliasEntry)
      expect(WriterOracle.write(entry.decl)).to eq("class CAlias = Foo\n")
    end
  end

  it "materializes a ModuleAlias declaration" do
    src = <<~RBS
      module M
      end
      module MAlias = M
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_alias_decls[RBS::TypeName.parse("::MAlias")]
      expect(entry).to be_a(RBS::Environment::ModuleAliasEntry)
      expect(WriterOracle.write(entry.decl)).to eq("module MAlias = M\n")
    end
  end

  it "preserves open-class declaration order in each_decl" do
    src = <<~RBS
      class Foo
        def a: () -> Integer
      end
      class Foo
        def b: () -> String
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(entry.each_decl.to_a.size).to eq(2)
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Foo
          def a: () -> Integer
        end
        class Foo
          def b: () -> String
        end
      RBS
    end
  end

  it "records nested decls under their fully-qualified name" do
    src = <<~RBS
      module Outer
        class Inner
          def x: () -> Integer
        end
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Outer::Inner")]
      expect(entry).to be_a(RBS::Environment::ClassEntry)
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Inner
          def x: () -> Integer
        end
      RBS
    end
  end

  # ----- AST::Members::* coverage -----
  #
  # The eight Declarations branches above land each AST::Members
  # variant on the dispatch table only via `MethodDefinition`. The
  # examples below pin every member kind we materialize so a regression
  # in `materialize/member.rs` shows up in the Writer string immediately.

  it "materializes mixin members (include / extend / prepend)" do
    src = <<~RBS
      module M
      end
      class Foo
        include M
        extend M
        prepend M
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Foo
          include M
          extend M
          prepend M
        end
      RBS
    end
  end

  it "materializes attribute members (attr_reader / attr_writer / attr_accessor)" do
    src = <<~RBS
      class Foo
        attr_reader r: Integer
        attr_writer w: String
        attr_accessor a: Float
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Foo
          attr_reader r: Integer
          attr_writer w: String
          attr_accessor a: Float
        end
      RBS
    end
  end

  it "materializes variable members (instance / class / class-instance)" do
    src = <<~RBS
      class Foo
        @ivar: Integer
        @@civar: String
        self.@cvar: bool
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Foo
          @ivar: Integer
          @@civar: String
          self.@cvar: bool
        end
      RBS
    end
  end

  it "materializes visibility markers and alias" do
    src = <<~RBS
      class Foo
        public
        def pm: () -> void

        private
        def vm: () -> void

        alias old new
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Foo
          public
          def pm: () -> void

          private
          def vm: () -> void

          alias old new
        end
      RBS
    end
  end

  # ----- Types::* coverage -----
  #
  # Each example targets one `RBS::Types::*` variant via a type alias
  # body (the simplest carrier: no member layer in between). The
  # Writer renders types through `Type#to_s`, so a regression in
  # materialize/type_.rs surfaces as a divergent right-hand side.

  it "materializes Types::Union / Intersection / Optional" do
    src = <<~RBS
      type u = Integer | String | nil
      type i = Integer & String
      type o = Integer?
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      decls = %w[::u ::i ::o].map { |n| env.type_alias_decls[RBS::TypeName.parse(n)].decl }
      expect(WriterOracle.write(decls[0])).to eq("type u = Integer | String | nil\n")
      expect(WriterOracle.write(decls[1])).to eq("type i = Integer & String\n")
      expect(WriterOracle.write(decls[2])).to eq("type o = Integer?\n")
    end
  end

  it "materializes Types::Tuple and Types::Record" do
    src = <<~RBS
      type t = [Integer, String, bool]
      type r = { name: String, age: Integer }
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      tuple_decl = env.type_alias_decls[RBS::TypeName.parse("::t")].decl
      record_decl = env.type_alias_decls[RBS::TypeName.parse("::r")].decl
      expect(WriterOracle.write(tuple_decl)).to eq("type t = [ Integer, String, bool ]\n")
      expect(WriterOracle.write(record_decl)).to eq("type r = { name: String, age: Integer }\n")
    end
  end

  it "materializes Types::Proc with positional / optional / rest / keyword params and a block" do
    src = <<~RBS
      type t = ^(Integer, ?String, *bool, foo: Symbol) { (Integer) -> void } -> Float
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      decl = env.type_alias_decls[RBS::TypeName.parse("::t")].decl
      expect(WriterOracle.write(decl)).to eq(
        "type t = ^(Integer, ?String, *bool, foo: Symbol) { (Integer) -> void } -> Float\n"
      )
    end
  end

  it "materializes Types::Literal across primitive kinds" do
    src = <<~RBS
      type t = 1 | "hello" | :sym | true | false | nil
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      decl = env.type_alias_decls[RBS::TypeName.parse("::t")].decl
      expect(WriterOracle.write(decl)).to eq(%(type t = 1 | "hello" | :sym | true | false | nil\n))
    end
  end

  it "materializes Types::Bases (bool / top / bot / nil / untyped)" do
    src = <<~RBS
      type t = bool | top | bot | nil | untyped
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      decl = env.type_alias_decls[RBS::TypeName.parse("::t")].decl
      expect(WriterOracle.write(decl)).to eq("type t = bool | top | bot | nil | untyped\n")
    end
  end

  it "materializes context-only Bases (self / instance / class / void)" do
    # `void` and `self` / `instance` / `class` are only legal in
    # method-return position, not inside a union, so they live here
    # rather than in the Bases test above.
    src = <<~RBS
      class Foo
        def s: () -> self
        def i: () -> instance
        def c: () -> class
        def v: () -> void
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Foo
          def s: () -> self
          def i: () -> instance
          def c: () -> class
          def v: () -> void
        end
      RBS
    end
  end

  it "materializes Types::ClassSingleton, ::ClassInstance, and ::Interface refs" do
    src = <<~RBS
      interface _Each
      end
      type cs = singleton(Integer)
      type ci = Array[Integer]
      type ifc = _Each
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      cs = env.type_alias_decls[RBS::TypeName.parse("::cs")].decl
      ci = env.type_alias_decls[RBS::TypeName.parse("::ci")].decl
      ifc = env.type_alias_decls[RBS::TypeName.parse("::ifc")].decl
      expect(WriterOracle.write(cs)).to eq("type cs = singleton(Integer)\n")
      expect(WriterOracle.write(ci)).to eq("type ci = Array[Integer]\n")
      expect(WriterOracle.write(ifc)).to eq("type ifc = _Each\n")
    end
  end

  # ----- AST::TypeParam variance / bound / unchecked -----

  it "materializes AST::TypeParam variants (variance, bound, unchecked)" do
    src = <<~RBS
      class Box[T, in U, out V, W < Numeric, unchecked X]
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Box")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Box[T, in U, out V, W < Numeric, unchecked X]
        end
      RBS
    end
  end

  # ----- MethodType -----

  it "materializes MethodType with type params, all parameter kinds, and an optional block" do
    src = <<~RBS
      class Foo
        def m: [T] (Integer x, ?String y, *Symbol z, foo: bool, ?bar: untyped, **untyped) ?{ (Integer) -> void } -> T
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        class Foo
          def m: [T] (Integer x, ?String y, *Symbol z, foo: bool, ?bar: untyped, **untyped) ?{ (Integer) -> void } -> T
        end
      RBS
    end
  end

  # ----- decl-head fields beyond name (super_class, self_types, annotations, comment) -----

  it "materializes a Class with super_class (plain and generic)" do
    src = <<~RBS
      class Base
      end
      class Container[T]
      end
      class Sub < Base
      end
      class Wrapper[T] < Container[T]
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      sub = env.class_decls[RBS::TypeName.parse("::Sub")]
      wrap = env.class_decls[RBS::TypeName.parse("::Wrapper")]
      expect(WriterOracle.write(sub.each_decl.to_a)).to eq("class Sub < Base\nend\n")
      expect(WriterOracle.write(wrap.each_decl.to_a)).to eq("class Wrapper[T] < Container[T]\nend\n")
    end
  end

  it "materializes a Module with self_types" do
    src = <<~RBS
      interface _Each
      end
      module M : _Each, Comparable
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::M")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq("module M : _Each, Comparable\nend\n")
    end
  end

  it "materializes annotations and a leading comment on a decl" do
    src = <<~RBS
      # class doc
      %a{annotation}
      class Foo
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      entry = env.class_decls[RBS::TypeName.parse("::Foo")]
      expect(WriterOracle.write(entry.each_decl.to_a)).to eq(<<~RBS)
        # class doc
        %a{annotation}
        class Foo
        end
      RBS
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
end
