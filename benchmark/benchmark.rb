# frozen_string_literal: true

# Measures `from_loader` + `resolve_type_names` + full materialization —
# the full "give me a usable RBS::Environment" pipeline as a Ruby caller
# sees it.
#
#     bundle exec ruby benchmark/benchmark.rb

require_relative "helpers"

EXPR = <<~RUBY
  env = RBS::Environment.from_loader(loader).resolve_type_names
  env.class_decls.size
RUBY

BenchHelpers.report_realtime(title: "cold-start (load + resolve)", expr: EXPR)
