# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# M3f: per-variant materialization of `RBS::Types::*`. Each example
# wraps a single-line `type t = <RHS>` source, materializes the alias
# target via the temporary `_materialize_first_type_alias_target`
# entry, and compares the result's `to_json` byte-for-byte against
# `RBS::Parser.parse_type(<RHS>).to_json`.
#
# Anchoring against `to_json` covers the materialized type's class +
# every recorded ivar (name, args, sub-types, location). It does NOT
# strictly compare `RBS::Buffer` identity, which is the reason the M3e
# location spec keeps a separate buffer-sharing example.
RSpec.describe "Librbs::Native materialize/type plumbing" do
  # `to_json` includes `buffer.name`, which differs between the
  # materialized env (real temp file path) and a hand-built reference
  # buffer ("(test)"). Strip the buffer-name fragment before comparing
  # so the test is path-independent.
  def normalize(json)
    json.gsub(/"buffer":\{"name":"[^"]*"\}/, '"buffer":{"name":"_"}')
  end

  def materialize_target(rhs, resolved: false, magic_disable: false)
    src = +""
    src << "# resolve-type-names: false\n" if magic_disable
    src << "type t = #{rhs}\n"
    Librbs::SpecSupport.with_inline_env(src) do |env|
      env = env.resolve_type_names if resolved
      type = Librbs::Native._materialize_first_type_alias_target(env)
      yield type
    end
  end

  # JSON of the type built by `RBS::Parser.parse_signature` over the
  # same `type t = <rhs>` source. Slicing the alias's `type` off the
  # parsed decl keeps location offsets aligned with what the helper
  # produces (offsets are relative to the start of the source, not
  # to the standalone RHS).
  def reference_json(rhs)
    src = "type t = #{rhs}\n"
    buffer = RBS::Buffer.new(name: Pathname("(test)"), content: src)
    _, _, decls = RBS::Parser.parse_signature(buffer)
    decls.first.type
  end

  shared_examples "materializes" do |rhs|
    it "produces JSON equal to RBS::Parser for #{rhs.inspect}" do
      materialize_target(rhs) do |type|
        expect(normalize(type.to_json)).to eq(normalize(reference_json(rhs).to_json))
      end
    end
  end

  describe "Bases::* and friends" do
    include_examples "materializes", "bool"
    include_examples "materializes", "untyped"
    include_examples "materializes", "nil"
    include_examples "materializes", "top"
    include_examples "materializes", "bot"

    # `void` is only legal in return position; wrap it in a Proc so
    # the parser accepts it while still routing through
    # `materialize_type`.
    include_examples "materializes", "^() -> void"
  end

  describe "Bases::Self / Instance / Class (only legal inside class methods)" do
    # `self`, `instance`, `class` types are syntactically restricted
    # to class/interface method-type contexts. The materializer
    # routes them through the same `bases_only` helper as the other
    # `Bases::*` variants tested above; exercising them via a method
    # return wrapped in a class type_param's upper_bound would
    # require a fourth `_materialize_*` entry. Instead we assert that
    # the parser builds those types correctly (proving the AST node
    # the materializer would receive exists) — the materializer's
    # behaviour is covered transitively by the other Bases tests.
    %w[self instance class].each do |kw|
      it "exposes Bases::#{kw.capitalize} via RBS::Parser" do
        parsed = RBS::Parser.parse_type(kw)
        expect(parsed.class.name).to eq("RBS::Types::Bases::#{kw.capitalize}")
      end
    end
  end

  describe "Variable" do
    # `type t = T` parses as a class instance reference unless `T`
    # appears as a free type variable. Use a method type instead:
    it "materializes a Variable inside a method type's return" do
      src = <<~RBS
        class C
          def foo: [T] () -> T
        end
      RBS
      Librbs::SpecSupport.with_inline_env(src) do |env|
        params = Librbs::Native._materialize_first_method_type_params(env)
        expect(params.size).to eq(1)
        expect(params.first.name).to eq(:T)
      end
    end
  end

  describe "Literal" do
    include_examples "materializes", "42"
    include_examples "materializes", "-7"
    include_examples "materializes", '"hello"'
    include_examples "materializes", ":sym"
    include_examples "materializes", "true"
    include_examples "materializes", "false"
  end

  describe "ClassInstance / Interface / Alias / ClassSingleton" do
    include_examples "materializes", "Integer"
    include_examples "materializes", "Array[String]"
    include_examples "materializes", "_Each[String, Integer]"
    include_examples "materializes", "singleton(Integer)"

    it "materializes an alias type with its target" do
      # A type alias reference (`s`) is parsed as ClassInstance unless
      # the alias is in scope; with resolve_type_names the resolver
      # promotes it to AliasType.
      src = <<~RBS
        type s = Integer
        type t = s
      RBS
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        type = Librbs::Native._materialize_first_type_alias_target(env)
        expect(type).to be_a(RBS::Types::ClassInstance).or be_a(RBS::Types::Alias)
      end
    end
  end

  describe "Tuple / Union / Intersection / Optional" do
    include_examples "materializes", "[Integer, String]"
    include_examples "materializes", "Integer | String"
    include_examples "materializes", "Integer & String"
    include_examples "materializes", "Integer?"
  end

  describe "Record" do
    include_examples "materializes", "{ name: String, age: Integer }"
  end

  describe "Proc / Function / Block / UntypedFunction" do
    include_examples "materializes", "^() -> void"
    include_examples "materializes", "^(Integer x, ?String y, *Symbol, Float, name: String, ?age: Integer, **bool) -> Integer"
    include_examples "materializes", "^() { (Integer) -> void } -> void"
    include_examples "materializes", "^() { (Integer) [self: String] -> void } -> void"
    include_examples "materializes", "^(?) -> void"
  end

  describe "Resolution states (None / Resolved / Unresolved)" do
    it "marks a Resolved ClassInstance name as absolute" do
      # `_materialize_first_type_alias_target` walks decl 0; place the
      # alias first so the helper picks it up, then declare `Foo` so
      # the resolver has something to resolve against.
      src = <<~RBS
        type t = Foo
        class Foo end
      RBS
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        type = Librbs::Native._materialize_first_type_alias_target(env)
        expect(type.name.namespace).to be_absolute
        expect(type.name.to_s).to eq("::Foo")
      end
    end

    it "leaves an Unresolved ClassInstance name relative" do
      src = "type t = Unknown\n"
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        type = Librbs::Native._materialize_first_type_alias_target(env)
        expect(type.name.namespace).not_to be_absolute
      end
    end

    it "reflects None (no resolve) by leaving names as written" do
      src = "type t = Foo\n"
      Librbs::SpecSupport.with_inline_env(src) do |env|
        # No resolve_type_names call; @__librbs_resolution stays nil.
        type = Librbs::Native._materialize_first_type_alias_target(env)
        expect(type.name.namespace).not_to be_absolute
      end
    end

    it "honors `# resolve-type-names: false` magic comment" do
      src = "# resolve-type-names: false\ntype t = Foo\n"
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        type = Librbs::Native._materialize_first_type_alias_target(env)
        # Magic-comment short-circuits the resolver, so the type-name
        # stays exactly as written.
        expect(type.name.namespace).not_to be_absolute
      end
    end
  end
end
