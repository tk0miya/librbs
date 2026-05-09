# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3g: per-shape materialization of `RBS::MethodType`. Each example
# wraps the method-type text in a one-method class, materializes via
# the temporary `_materialize_first_method_type` entry, and compares
# `to_json` byte-for-byte against `RBS::Parser.parse_method_type`.
RSpec.describe "Librbs::Native materialize/method_type plumbing" do
  def normalize(json)
    json.gsub(/"buffer":\{"name":"[^"]*"\}/, '"buffer":{"name":"_"}')
  end

  def materialize_method_type(method_type_text)
    src = <<~RBS
      class C
        def foo: #{method_type_text}
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      yield Librbs::Native._materialize_first_method_type(env)
    end
  end

  def reference_method_type(method_type_text)
    src = <<~RBS
      class C
        def foo: #{method_type_text}
      end
    RBS
    buffer = RBS::Buffer.new(name: Pathname("(test)"), content: src)
    _, _, decls = RBS::Parser.parse_signature(buffer)
    method = decls.first.members.find { |m| m.is_a?(RBS::AST::Members::MethodDefinition) }
    method.overloads.first.method_type
  end

  shared_examples "method-type" do |label, text|
    it "matches RBS::Parser for #{label}" do
      materialize_method_type(text) do |mt|
        expect(normalize(mt.to_json)).to eq(normalize(reference_method_type(text).to_json))
      end
    end
  end

  describe "plain method type" do
    include_examples "method-type", "no args", "() -> void"
    include_examples "method-type", "one positional", "(Integer) -> String"
    include_examples "method-type", "kwargs and rest", "(*Integer, name: String, ?age: Integer) -> void"
  end

  describe "type_params" do
    include_examples "method-type", "single param", "[T] (T) -> T"
    include_examples "method-type", "bound", "[T < Numeric] (T) -> T"
    include_examples "method-type", "two params", "[T, U] (T, U) -> [T, U]"
  end

  describe "block" do
    include_examples "method-type", "required block", "() { (Integer) -> String } -> void"
    include_examples "method-type", "optional block", "() ?{ (Integer) -> String } -> void"
    include_examples "method-type", "self-bound block", "() { (Integer) [self: String] -> void } -> void"
  end

  describe "type_params + block combined" do
    include_examples "method-type", "[T] with block", "[T] () { (T) -> void } -> T"
  end
end
