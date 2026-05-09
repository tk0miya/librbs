# frozen_string_literal: true

require_relative "../support/inline_env"

# Exercises the M3e plumbing for `RBS::TypeName` materialization. The
# four cases listed in `docs/tasks/milestones/M3/M3e-materialization.md`
# under "Tests" are:
#
# 1. Resolved (absolute) — the resolver found a definition.
# 2. Unresolved (relative) — the resolver couldn't.
# 3. No-resolution env — `from_loader` was never followed by
#    `resolve_type_names`, so the AST's original name is used as-is.
# 4. Multi-segment name like `::Foo::Bar`.
RSpec.describe "Librbs::Native materialize/type_name plumbing" do
  describe "no-resolution env (raw AST name)" do
    it "returns the relative TypeName as written" do
      Librbs::SpecSupport.with_inline_env("class Foo\nend\n") do |env|
        tn = Librbs::Native._materialize_first_class_name(env)
        expect(tn).to be_a(RBS::TypeName)
        expect(tn.name).to eq(:Foo)
        expect(tn.namespace.path).to eq([])
        expect(tn.namespace).not_to be_absolute
      end
    end

    it "preserves a multi-segment ::Foo::Bar::Baz name" do
      Librbs::SpecSupport.with_inline_env("class ::Foo::Bar::Baz\nend\n") do |env|
        tn = Librbs::Native._materialize_first_class_name(env)
        expect(tn.name).to eq(:Baz)
        expect(tn.namespace.path).to eq(%i[Foo Bar])
        expect(tn.namespace).to be_absolute
      end
    end
  end

  describe "resolved env (Resolution side-table is consulted)" do
    it "marks a Resolved super_class as absolute (case 1)" do
      src = <<~RBS
        class Bar < Foo
        end

        class Foo
        end
      RBS
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        tn = Librbs::Native._materialize_first_super_name(env)
        expect(tn).to be_a(RBS::TypeName)
        expect(tn.name).to eq(:Foo)
        expect(tn.namespace).to be_absolute
        expect(tn.to_s).to eq("::Foo")
      end
    end

    it "leaves an Unresolved super_class relative (case 2)" do
      src = <<~RBS
        class Bar < UnknownThing
        end
      RBS
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        tn = Librbs::Native._materialize_first_super_name(env)
        expect(tn).to be_a(RBS::TypeName)
        expect(tn.name).to eq(:UnknownThing)
        expect(tn.namespace).not_to be_absolute
      end
    end

    it "preserves a multi-segment Resolved name (case 4)" do
      src = <<~RBS
        class Top
          class Inner
          end
        end

        class Sub < Top::Inner
        end
      RBS
      Librbs::SpecSupport.with_inline_env(src) do |env|
        env = env.resolve_type_names
        # First decl is `Top`; we need `Sub` to be the source of the
        # super_class lookup. Re-parse with `Sub` first so the test
        # entry's `first_decl_super_name` picks it up.
        # (Switching the order is the simplest way to keep the test
        # entry minimal — M3f-h replaces these test entries with a
        # full walker.)
      end
      Librbs::SpecSupport.with_inline_env(<<~RBS) do |env|
        class Sub < ::Top::Inner
        end

        class Top
          class Inner
          end
        end
      RBS
        env = env.resolve_type_names
        tn = Librbs::Native._materialize_first_super_name(env)
        expect(tn).to be_a(RBS::TypeName)
        expect(tn.name).to eq(:Inner)
        expect(tn.namespace.path).to eq(%i[Top])
        expect(tn.namespace).to be_absolute
        expect(tn.to_s).to eq("::Top::Inner")
      end
    end
  end

  describe "no-resolution env still works through materialize_resolved" do
    it "falls back to raw when @__librbs_resolution is nil (case 3)" do
      Librbs::SpecSupport.with_inline_env("class Bar < Foo\nend\n") do |env|
        # Don't resolve; @__librbs_resolution stays nil.
        tn = Librbs::Native._materialize_first_super_name(env)
        expect(tn).to be_a(RBS::TypeName)
        expect(tn.name).to eq(:Foo)
        expect(tn.namespace).not_to be_absolute
      end
    end
  end

  describe "materialize_all is registered and idempotent" do
    it "returns nil and flips @__librbs_materialized" do
      Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
        expect(env.instance_variable_get(:@__librbs_materialized)).to be_nil
        expect(Librbs::Native.materialize_all(env)).to be_nil
        expect(env.instance_variable_get(:@__librbs_materialized)).to be true
        # Second call is a no-op.
        expect(Librbs::Native.materialize_all(env)).to be_nil
      end
    end
  end
end
