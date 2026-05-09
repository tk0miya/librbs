# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3g: per-variant materialization of `RBS::AST::Members::*`. Each
# example wraps the member text in a one-class container, materializes
# via the temporary `_materialize_first_member` entry, and compares
# `to_json` byte-for-byte against `RBS::Parser.parse_signature`.
RSpec.describe "Librbs::Native materialize/member plumbing" do
  def normalize(json)
    json.gsub(/"buffer":\{"name":"[^"]*"\}/, '"buffer":{"name":"_"}')
  end

  # Wrap `body` in `class C ... end` (or another container kind) and
  # materialize the first member. `prelude` lets a test add a
  # preceding line (e.g. an annotation, comment, or sibling decl) so
  # the member-of-interest stays the first member of the first decl.
  def materialize_member(body, container: "class C", resolved: false, prelude: nil)
    src = +""
    src << "#{prelude}\n" if prelude
    src << "#{container}\n#{body.lines.map { |l| "  #{l}" }.join}\nend\n"
    Librbs::SpecSupport.with_inline_env(src) do |env|
      env = env.resolve_type_names if resolved
      yield Librbs::Native._materialize_first_member(env), src
    end
  end

  def reference_member(src)
    buffer = RBS::Buffer.new(name: Pathname("(test)"), content: src)
    _, _, decls = RBS::Parser.parse_signature(buffer)
    decls.first.members.find { |m| !m.is_a?(RBS::AST::Declarations::Base) } ||
      decls.first.members.first
  end

  shared_examples "member" do |label, body, container: "class C", resolved: false, prelude: nil|
    it "matches RBS::Parser for #{label}" do
      materialize_member(body, container: container, resolved: resolved, prelude: prelude) do |mat, src|
        expect(normalize(mat.to_json)).to eq(normalize(reference_member(src).to_json))
      end
    end
  end

  describe "MethodDefinition" do
    include_examples "member", "instance method", "def foo: () -> void"
    include_examples "member", "singleton method", "def self.bar: () -> Integer"
    include_examples "member",
                    "self?. (singleton_instance)",
                    "def self?.baz: () -> Integer"
    include_examples "member", "private visibility", "private def foo: () -> void"
    include_examples "member", "public visibility", "public def foo: () -> void"
    include_examples "member",
                    "overloading (...)",
                    "def foo: () -> void\n        | ...",
                    container: "class C"
    include_examples "member", "with annotation", "%a{pure}\n  def foo: () -> void"
    include_examples "member",
                    "with leading comment",
                    "# leading\n  def foo: () -> void"
    include_examples "member",
                    "two overloads",
                    "def foo: () -> void\n        | (Integer) -> String"
  end

  describe "AttrAccessor / AttrReader / AttrWriter" do
    include_examples "member",
                    "attr_reader Unspecified ivar_name",
                    "attr_reader name: String"
    include_examples "member",
                    "attr_accessor Empty ivar_name (no-store)",
                    "attr_accessor age (): Integer"
    include_examples "member",
                    "attr_writer named ivar_name",
                    "attr_writer email (@e): String"
    include_examples "member", "singleton attr_reader", "attr_reader self.tag: Symbol"
    include_examples "member", "private attr_accessor", "private attr_accessor name: String"
  end

  describe "Var members" do
    include_examples "member", "InstanceVariable", "@count: Integer"
    include_examples "member", "ClassInstanceVariable", "self.@count: Integer"
    include_examples "member", "ClassVariable", "@@count: Integer"
  end

  describe "Mixin members (Include / Extend / Prepend)" do
    include_examples "member", "include unresolved", "include Unknown"
    include_examples "member", "extend unresolved", "extend Unknown"
    include_examples "member", "prepend unresolved", "prepend Unknown"
    include_examples "member",
                    "include with args",
                    "include Enumerable[Integer, Integer]"

    it "absolutizes a resolved Include name" do
      src = "module M\nend\nclass C\n  include M\nend\n"
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        # First decl is `module M`; second is `class C`. The first
        # member there is the `include M`. The helper walks decl 0,
        # but here we want the include from decl 1. Build a custom
        # source where the class with the mixin is decl 0 instead:
      end
      src2 = "class C\n  include M\nend\nmodule M\nend\n"
      Librbs::SpecSupport.with_inline_env(src2) do |env|
        env = env.resolve_type_names
        mat = Librbs::Native._materialize_first_member(env)
        expect(mat).to be_a(RBS::AST::Members::Include)
        expect(mat.name.namespace).to be_absolute
        expect(mat.name.to_s).to eq("::M")
      end
    end

    it "leaves an unresolved Include relative" do
      src = "class C\n  include Unknown\nend\n"
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        mat = Librbs::Native._materialize_first_member(env)
        expect(mat).to be_a(RBS::AST::Members::Include)
        expect(mat.name.namespace).not_to be_absolute
      end
    end
  end

  describe "Alias" do
    include_examples "member", "instance alias", "alias foo bar"
    include_examples "member", "singleton alias", "alias self.foo self.bar"
  end

  describe "Public / Private (location-only)" do
    include_examples "member", "public bare", "public"
    include_examples "member", "private bare", "private"
  end
end
