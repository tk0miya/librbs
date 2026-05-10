# frozen_string_literal: true

require_relative "../support/writer_oracle"

# M3j: per-decl Writer parity for a curated core set. Complements the
# bulk canonical_dump matrix in `core_spec.rb` /
# `core_stdlib_spec.rb` / `gems_spec.rb`. Where canonical_dump checks
# the six tables fill correctly, this spec checks each entry's
# materialized AST round-trips through `RBS::Writer` to the same
# user-visible RBS syntax that pure RBS would produce. Resolved
# variants are where the Writer-based oracle earns its keep — `to_s`
# on absolute type references shows up in the printed output, and
# materialization is responsible for keeping them in sync.
RSpec.describe "RBS::Writer per-decl compatibility (core)" do
  CORE_NAMES = %w[
    ::Object
    ::Integer
    ::String
    ::Array
    ::Hash
    ::Numeric
  ].freeze

  UNRESOLVED_ENV_SCRIPT = <<~RUBY
    env = RBS::Environment.from_loader(RBS::EnvironmentLoader.new)
  RUBY

  RESOLVED_ENV_SCRIPT = <<~RUBY
    env = RBS::Environment.from_loader(RBS::EnvironmentLoader.new).resolve_type_names
  RUBY

  CORE_NAMES.each do |name_str|
    it "matches pure RBS for unresolved #{name_str}" do
      env = RBS::Environment.from_loader(RBS::EnvironmentLoader.new)
      type_name = RBS::TypeName.parse(name_str)
      entry = env.class_decls.fetch(type_name)
      librbs_str = Librbs::SpecSupport::WriterOracle.write(entry.each_decl.to_a)
      pure_str = Librbs::SpecSupport::WriterOracle.write_pure(UNRESOLVED_ENV_SCRIPT, type_name)
      expect(librbs_str).to eq(pure_str)
    end

    it "matches pure RBS for resolved #{name_str}" do
      env = RBS::Environment.from_loader(RBS::EnvironmentLoader.new).resolve_type_names
      type_name = RBS::TypeName.parse(name_str)
      entry = env.class_decls.fetch(type_name)
      librbs_str = Librbs::SpecSupport::WriterOracle.write(entry.each_decl.to_a)
      pure_str = Librbs::SpecSupport::WriterOracle.write_pure(RESOLVED_ENV_SCRIPT, type_name)
      expect(librbs_str).to eq(pure_str)
    end
  end
end
