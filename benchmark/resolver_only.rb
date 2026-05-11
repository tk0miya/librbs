# frozen_string_literal: true

# Isolates resolver cost by building the environment once (untimed),
# then timing N rounds of `RBS::Resolver::TypeNameResolver` queries
# against every declared name. Removes the parse-time noise that masks
# resolver-only deltas in `three_way_resolve.rb`.
#
# librbs is excluded here because it replaces `Environment#resolve_type_names`
# at the top level — there is no comparable single-name `resolve` entry
# point on the Rust side.
#
#     bundle exec ruby benchmark/resolver_only.rb

require_relative "helpers"

EXPR = <<~RUBY
  # Build env once, untimed. The full resolve_type_names walks every
  # declaration and every type reference inside member signatures /
  # supertypes / aliases, so it is the realistic resolver workload.
  env = RBS::Environment.from_loader(loader)
  GC.start
  t = Benchmark.realtime do
    env.resolve_type_names
  end
  t
RUBY

# Custom driver: the helper's `measure_realtime` builds the loader inside
# the timed block, which we don't want here. Inline a smaller version.
module BenchHelpers
  module_function

  def measure_resolver_only(impl:, size:, repeats: 5)
    body = <<~RUBY
      require "benchmark"
      times = []
      #{repeats}.times do
        #{loader_setup(size)}
        #{EXPR}
        times << t
      end
      puts times.min
    RUBY
    run_subprocess(impl: impl, body: body).strip.to_f
  end
end

impls = %i[pure_rbs rbs_patched rbs_patched_v2]
sizes = BenchHelpers::SIZES.keys

puts "## resolver_only.rb"
puts
measurements = sizes.each_with_object({}) do |size, h|
  h[size] = impls.each_with_object({}) do |impl, ih|
    ih[impl] = BenchHelpers.measure_resolver_only(impl: impl, size: size)
  end
end

headers = ["size"] + impls.map(&:to_s) + (impls - [:pure_rbs]).map { |i| "#{i} speedup" }
rows = sizes.map do |size|
  base = measurements[size][:pure_rbs]
  cells = [size.to_s] + impls.map { |i| BenchHelpers.format_ms(measurements[size][i]) }
  cells += (impls - [:pure_rbs]).map { |i| BenchHelpers.format_speedup(base, measurements[size][i]) }
  cells
end
BenchHelpers.print_table(headers, rows)
puts
