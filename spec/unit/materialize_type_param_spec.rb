# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3f: per-variant materialization of `RBS::AST::TypeParam`. Two
# harnesses are needed because variance and `unchecked` are
# declaration-level only, while bounds and defaults are valid in both
# method-type and declaration-level type_params.
RSpec.describe "Librbs::Native materialize/type_param plumbing" do
  def normalize(json)
    json.gsub(/"buffer":\{"name":"[^"]*"\}/, '"buffer":{"name":"_"}')
  end

  # ---- method-type harness ----

  def materialize_method_params(method_type_text)
    src = <<~RBS
      class C
        def foo: #{method_type_text}
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      yield Librbs::Native._materialize_first_method_type_params(env)
    end
  end

  def reference_method_params(method_type_text)
    src = <<~RBS
      class C
        def foo: #{method_type_text}
      end
    RBS
    buffer = RBS::Buffer.new(name: Pathname("(test)"), content: src)
    _, _, decls = RBS::Parser.parse_signature(buffer)
    method = decls.first.members.find { |m| m.is_a?(RBS::AST::Members::MethodDefinition) }
    method.overloads.first.method_type.type_params
  end

  shared_examples "method-level type_params" do |method_type|
    it "matches RBS::Parser for #{method_type.inspect}" do
      materialize_method_params(method_type) do |arr|
        expect(arr.map { |p| normalize(p.to_json) })
          .to eq(reference_method_params(method_type).map { |p| normalize(p.to_json) })
      end
    end
  end

  # ---- declaration-level harness ----

  def materialize_class_params(type_params_text, kind: "class")
    src = "#{kind} C[#{type_params_text}]\nend\n"
    Librbs::SpecSupport.with_inline_env(src) do |env|
      yield Librbs::Native._materialize_first_class_type_params(env)
    end
  end

  def reference_class_params(type_params_text, kind: "class")
    src = "#{kind} C[#{type_params_text}]\nend\n"
    buffer = RBS::Buffer.new(name: Pathname("(test)"), content: src)
    _, _, decls = RBS::Parser.parse_signature(buffer)
    decls.first.type_params
  end

  shared_examples "decl-level type_params" do |label, params, kind: "class"|
    it "matches RBS::Parser for #{label}" do
      materialize_class_params(params, kind: kind) do |arr|
        expect(arr.map { |p| normalize(p.to_json) })
          .to eq(reference_class_params(params, kind: kind).map { |p| normalize(p.to_json) })
      end
    end
  end

  describe "variance" do
    include_examples "decl-level type_params", "out T", "out T"
    include_examples "decl-level type_params", "in U", "in U"
    include_examples "decl-level type_params", "T (invariant default)", "T"
  end

  describe "unchecked modifier" do
    include_examples "decl-level type_params", "unchecked out T", "unchecked out T"
    include_examples "decl-level type_params", "unchecked T", "unchecked T"
  end

  describe "bounds (method-level)" do
    include_examples "method-level type_params", "[T < Numeric] () -> T"
    include_examples "method-level type_params", "[T] () -> T"
  end

  describe "default (decl-level)" do
    # Defaults `[T = Foo]` are decl-level only.
    include_examples "decl-level type_params", "T = Numeric", "T = Numeric"
  end

  describe "multiple params" do
    include_examples "method-level type_params", "[T, U < _Each[T]] () -> [T, U]"
    include_examples "decl-level type_params", "T, out U, unchecked in V", "T, out U, unchecked in V"
  end
end
