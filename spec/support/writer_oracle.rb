# frozen_string_literal: true

require "stringio"
require "rbs"

require_relative "without_librbs"

# Writer-based oracle: print materialization-produced AST through
# `RBS::Writer` (non-preserve mode) and use the resulting string as
# the comparison point. Stronger than `is_a?` chains for unit specs
# (it pins down every printed field) and stronger than `to_json` for
# compat specs (it renders the user-visible RBS syntax we actually
# care about staying compatible with).
#
# Centralizing `RBS::Writer.new` here keeps the non-preserve invariant
# explicit. `Writer#write_loc_source` reads `loc.source` from the
# original parser buffer when `preserve?` is true; materialization
# produces synthetic locations whose buffer is the temp source the
# test reasons about, so `preserve!` mode would drift the output by
# whitespace and comments that don't belong in the comparison.
module Librbs
  module SpecSupport
    module WriterOracle
      # Print one decl through `RBS::Writer` (non-preserve mode) and
      # return the resulting String. Accepts a single decl or an Array
      # of decls (used for open classes via `ClassEntry#each_decl`).
      def self.write(decl_or_decls)
        decls = Array(decl_or_decls)
        io = StringIO.new
        RBS::Writer.new(out: io).write(decls)
        io.string
      end

      # Run the given env-build script in a fresh ruby subprocess
      # without librbs loaded. The script must leave an
      # `RBS::Environment` bound to a local named `env`. After the
      # script runs, the entry at `env.class_decls[type_name]` is
      # walked through `each_decl` and the result is fed to
      # `WriterOracle.write`. Returns the captured String.
      def self.write_pure(env_script, type_name)
        name_literal = type_name.to_s.inspect
        # The subprocess is running pure RBS which emits UTF-8
        # source (RBS comments include non-ASCII rdoc text); the
        # librbs side stores the same bytes as UTF-8. Open3 hands
        # back ASCII-8BIT, so retag the bytes as UTF-8 for `eq` to
        # compare on equal footing.
        out = without_librbs(<<~RUBY)
          require "rbs"
          require "stringio"

          #{env_script}

          name = RBS::TypeName.parse(#{name_literal})
          entry = env.class_decls.fetch(name)
          io = StringIO.new
          RBS::Writer.new(out: io).write(entry.each_decl.to_a)
          print io.string
        RUBY
        out.force_encoding(Encoding::UTF_8)
      end
    end
  end
end
