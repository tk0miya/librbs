# frozen_string_literal: true

# Measures `from_loader` followed by full materialization (no name
# resolution). `class_decls.size` triggers `Native.materialize_all` on
# the librbs path so we are comparing fully realized Ruby state on both
# sides — pure Rust-side work has no value to a Ruby caller.
#
#     bundle exec ruby benchmark/load_only.rb

require_relative "helpers"

EXPR = <<~RUBY
  env = RBS::Environment.from_loader(loader)
  env.class_decls.size
RUBY

BenchHelpers.report_realtime(title: "load_only.rb", expr: EXPR)
