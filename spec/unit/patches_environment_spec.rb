# frozen_string_literal: true

require "rbs"

require_relative "../support/inline_env"

# Pure-Ruby `RBS::Environment.new` instances have no `@__librbs_handle`
# and must continue to work through the patched accessors. The patches'
# `instance_variable_defined?` guard is what preserves that fallback —
# assert here that the guard holds and that
# `Librbs::Native.materialize_all` is never reached on this path.
RSpec.describe "Librbs::Patches::Environment fallback" do
  it "does not call into native materialization for a pure-Ruby env" do
    env = RBS::Environment.new
    expect(Librbs::Native).not_to receive(:materialize_all)

    expect(env.class_decls).to eq({})
    expect(env.interface_decls).to eq({})
    expect(env.type_alias_decls).to eq({})
    expect(env.constant_decls).to eq({})
    expect(env.class_alias_decls).to eq({})
    expect(env.global_decls).to eq({})

    expect(env.instance_variable_defined?(:@__librbs_handle)).to be(false)
    expect(env.instance_variable_get(:@__librbs_materialized)).to be_nil
  end

  it "leaves accessors functional after a follow-up call (no caching of a nil result)" do
    env = RBS::Environment.new
    first = env.class_decls
    second = env.class_decls
    expect(second).to equal(first)
  end
end

# Followup: complete materialize-trigger coverage on `RBS::Environment`.
# The accessors enumerated in `Librbs::Patches::Environment` are not
# the only state-reading methods upstream defines — `inspect`,
# `initialize_copy`, `add_source`, and `unload` read or mutate the
# ivars directly and would observe pre-materialization (empty) state
# without their own trigger.
RSpec.describe "Librbs::Patches::Environment trigger coverage" do
  it "`inspect` reports populated sizes before any other accessor runs" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      out = env.inspect
      expect(out).to match(/@class_decls=\(1 items\)/)
      expect(out).to match(/@sources=\(1 items\)/)
      expect(out).not_to match(/@class_decls=\(0 items\)/)
    end
  end

  it "`inspect` still reports populated sizes once materialization has already run" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      env.class_decls
      expect(env.inspect).to match(/@class_decls=\(1 items\)/)
    end
  end

  it "`dup` preserves the decl hashes when called before any other accessor" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      copy = env.dup
      expect(copy.class_decls).to eq(env.class_decls)
      expect(copy.class_decls).not_to be_empty
      # `Source::RBS` object identity matches across the dup, because
      # `initialize_copy` materializes `other` before upstream's
      # shallow Hash/Array dup runs.
      expect(copy.sources).to eq(env.sources)
      expect(copy.sources.first).to equal(env.sources.first)
    end
  end

  it "`dup` shares the source env's @__librbs_handle (Arc-shared core)" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      copy = env.dup
      expect(copy.instance_variable_get(:@__librbs_handle))
        .to equal(env.instance_variable_get(:@__librbs_handle))
      # And the inherited `@__librbs_materialized = true` flag keeps
      # the dup off the native re-materialization path.
      expect(Librbs::Native).not_to receive(:materialize_all)
      expect(copy.class_decls).not_to be_empty
    end
  end

  it "`dup` gives the dup its own decl Hashes (mutations don't leak)" do
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      copy = env.dup
      expect(copy.class_decls).not_to equal(env.class_decls)
      expect(copy.sources).not_to equal(env.sources)
    end
  end

  it "`add_source` called before any other accessor preserves the Rust-side decls" do
    extra_src = <<~RBS
      class Extra
      end
    RBS
    Librbs::SpecSupport.with_inline_env("class A end\n") do |env|
      buffer = RBS::Buffer.new(name: Pathname("extra.rbs"), content: extra_src)
      _, dirs, decls = RBS::Parser.parse_signature(buffer)
      env.add_source(RBS::Source::RBS.new(buffer, dirs, decls))

      keys = env.class_decls.keys.map(&:to_s)
      expect(keys).to include("::A")
      expect(keys).to include("::Extra")
    end
  end

  it "`unload` called before any other accessor preserves the Rust-side decls" do
    src = <<~RBS
      class A
      end
      class B
      end
    RBS
    Librbs::SpecSupport.with_inline_env(src) do |env|
      reduced = env.unload([])
      keys = reduced.class_decls.keys.map(&:to_s)
      expect(keys).to contain_exactly("::A", "::B")
    end
  end
end

# Meta-regression guard: every public instance method that upstream
# `RBS::Environment` defines must either be explicitly patched by
# `Librbs::Patches::Environment` or reach state only through patched
# accessors. A future upstream bump that introduces a new ivar-reading
# method fails this assertion instead of silently shipping a
# `(0 items)`-style bug.
RSpec.describe "Librbs::Patches::Environment meta-coverage" do
  # Methods that read state through the patched accessors only — no
  # direct ivar reads, no `instance_variable_get`. Each entry has
  # been audited against `vendor/rbs/lib/rbs/environment.rb`. New
  # methods on upstream must either land in this list (audited safe)
  # or in the patched-methods set in `Librbs::Patches::Environment`.
  AUDITED_SAFE = %i[
    buffers
    interface_name?
    type_alias_name?
    module_name?
    type_name?
    constant_name?
    constant_decl?
    class_decl?
    module_decl?
    module_alias?
    class_alias?
    class_entry
    module_entry
    module_class_entry
    constant_entry
    normalized_class_entry
    normalized_module_entry
    normalized_module_class_entry
    normalize_type_name?
    normalize_type_name
    normalize_type_name!
    normalized_type_name?
    normalized_type_name!
    normalize_module_name?
    normalize_module_name
    normalize_module_name!
    insert_rbs_decl
    insert_ruby_decl
    validate_type_params
    resolve_signature
    resolve_declaration
    resolve_member
    resolve_method_type
    resolve_ruby_decl
    resolve_ruby_member
    resolve_type_params
    resolver_context
    append_context
    absolute_type
    absolute_type_name
  ].freeze

  def patched_methods
    mod = Librbs::Patches::Environment
    mod.instance_methods(false) + mod.private_instance_methods(false)
  end

  def upstream_public_methods
    # `RBS::Environment.instance_methods(false)` after the prepend
    # still returns upstream-defined methods (prepended modules are
    # excluded by `false`). We list every public instance method
    # actually defined on the class body.
    RBS::Environment.instance_methods(false).sort
  end

  it "covers every public instance method of upstream RBS::Environment" do
    patched = patched_methods
    audited = AUDITED_SAFE

    uncovered = upstream_public_methods.reject do |m|
      patched.include?(m) || audited.include?(m)
    end

    expect(uncovered).to eq([]),
      "Unpatched/unaudited methods on RBS::Environment: #{uncovered.inspect}. " \
        "Either patch them in lib/librbs/patches/environment.rb or, if they " \
        "only read state via patched accessors, add them to AUDITED_SAFE."
  end

  it "does not list audited-safe methods that no longer exist upstream" do
    # If upstream removes or renames a method, the AUDITED_SAFE entry
    # is dead weight. Catch it here so the audit list stays bounded.
    upstream = upstream_public_methods
    stale = AUDITED_SAFE - upstream
    expect(stale).to eq([]),
      "AUDITED_SAFE references methods no longer on RBS::Environment: #{stale.inspect}"
  end
end
