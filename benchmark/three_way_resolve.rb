# frozen_string_literal: true

# Compares three implementations on `from_loader + resolve_type_names`:
#
#   - pure_rbs:    upstream RBS, no patches
#   - rbs_patched: upstream RBS + benchmark/rbs_intern_patch.rb
#                  (hash-cons RBS::TypeName / RBS::Namespace, memoize
#                  TypeName#to_namespace) — pure-Ruby ad-hoc PoC of the
#                  Rust interner approach, intended for upstream proposal.
#   - librbs:      Rust-backed Native.resolve_type_names
#
# The patched config is the focus of this benchmark: does the interner
# approach pay off in pure Ruby enough to be worth proposing upstream?
#
#     bundle exec ruby benchmark/three_way_resolve.rb

require_relative "helpers"

EXPR = <<~RUBY
  env = RBS::Environment.from_loader(loader).resolve_type_names
  env.class_decls.size
RUBY

BenchHelpers.report_realtime(title: "three_way_resolve.rb", expr: EXPR, repeats: 5)
