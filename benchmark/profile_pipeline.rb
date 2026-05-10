# frozen_string_literal: true

# Phase-level breakdown + stackprof sampling profile for the librbs
# load pipeline. Splits the pipeline into:
#
#   1. from_loader         (Rust: discover + parallel parse + serial insert)
#   2. materialize         (Rust → Ruby: build Source::RBS + add_source)
#   3. resolve_type_names  (Rust resolver) + its materialize
#
# Then re-runs the pipeline under stackprof to produce a sampling
# profile across both Ruby and the native extension.
#
# Usage:
#   bundle exec ruby benchmark/profile_pipeline.rb [size]
#
# size = small | medium | large (default: large)

$LOAD_PATH.unshift(File.expand_path("../lib", __dir__))
require "rbs"
require "librbs"
require "benchmark"
require "stackprof"

unless defined?(Librbs::Native) && Librbs::Native.respond_to?(:build_environment)
  abort "[bench] librbs native extension is not loaded; run `bundle exec rake compile` first"
end

SIZE = (ARGV[0] || "large").to_sym
REPEATS = (ARGV[1] || 5).to_i
STACKPROF_ITERS = (ARGV[2] || 30).to_i

MEDIUM_LIBS = %w[pathname date time uri optparse logger stringio strscan].freeze

def build_loader(size)
  loader = RBS::EnvironmentLoader.new
  case size
  when :small
    # core only
  when :medium
    MEDIUM_LIBS.each { |l| loader.add(library: l) }
  when :large
    stdlib_root = File.expand_path("../vendor/rbs/stdlib", __dir__)
    Dir.children(stdlib_root).sort.each do |name|
      path = File.join(stdlib_root, name)
      next unless File.directory?(path)
      loader.add(path: Pathname(path))
    end
  else
    raise ArgumentError, "unknown size #{size.inspect}"
  end
  loader
end

# Force materialize side: touching class_decls.size triggers
# materialize_all on the native path.
def force_materialize(env)
  env.class_decls.size
end

def measure_phases(size)
  loader = build_loader(size)
  GC.start

  t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  env = RBS::Environment.from_loader(loader)
  t1 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  force_materialize(env)
  t2 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  resolved = env.resolve_type_names
  t3 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  force_materialize(resolved)
  t4 = Process.clock_gettime(Process::CLOCK_MONOTONIC)

  {
    from_loader:     (t1 - t0) * 1000,
    materialize_pre: (t2 - t1) * 1000,
    resolve:         (t3 - t2) * 1000,
    materialize_res: (t4 - t3) * 1000,
    total:           (t4 - t0) * 1000,
  }
end

# ---------- phase timing ----------

puts "size=#{SIZE} repeats=#{REPEATS}"
runs = Array.new(REPEATS) { measure_phases(SIZE) }
mins = runs.first.keys.each_with_object({}) do |k, h|
  h[k] = runs.map { |r| r[k] }.min
end

format_ms = ->(v) { format("%8.2f ms", v) }
puts "\nWall-clock breakdown (min of #{REPEATS} repeats):"
puts "-" * 56
%i[from_loader materialize_pre resolve materialize_res total].each do |k|
  printf "  %-18s %s  (%5.1f%%)\n",
         k, format_ms.call(mins[k]), 100.0 * mins[k] / mins[:total]
end

# ---------- stackprof ----------

profile_path = File.expand_path("../stackprof_pipeline_#{SIZE}.dump", __dir__)
puts "\nRecording stackprof (#{STACKPROF_ITERS} iterations, mode=:wall, interval=500us)..."

StackProf.run(mode: :wall, raw: true, interval: 500, out: profile_path) do
  STACKPROF_ITERS.times do
    loader = build_loader(SIZE)
    env = RBS::Environment.from_loader(loader)
    force_materialize(env)
    resolved = env.resolve_type_names
    force_materialize(resolved)
  end
end

puts "wrote #{profile_path}"
puts
report = StackProf::Report.new(Marshal.load(File.binread(profile_path)))
puts "Top frames by total time:"
puts "-" * 56
report.print_text(false, 25)
