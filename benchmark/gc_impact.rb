# frozen_string_literal: true

# Measure GC invocations during the librbs load pipeline, and compare
# wall-clock with GC enabled vs. disabled. Also reports how much heap
# the GC-disabled run grew (to estimate the practicality of disabling
# GC during materialize).
#
# Usage:
#   bundle exec ruby benchmark/gc_impact.rb [size]

$LOAD_PATH.unshift(File.expand_path("../lib", __dir__))
require "rbs"
require "librbs"

SIZE = (ARGV[0] || "large").to_sym
REPEATS = (ARGV[1] || 5).to_i

MEDIUM_LIBS = %w[pathname date time uri optparse logger stringio strscan].freeze

def build_loader(size)
  loader = RBS::EnvironmentLoader.new
  case size
  when :small then nil
  when :medium then MEDIUM_LIBS.each { |l| loader.add(library: l) }
  when :large
    stdlib_root = File.expand_path("../vendor/rbs/stdlib", __dir__)
    Dir.children(stdlib_root).sort.each do |name|
      path = File.join(stdlib_root, name)
      next unless File.directory?(path)
      loader.add(path: Pathname(path))
    end
  end
  loader
end

def force_materialize(env)
  env.class_decls.size
end

def one_pipeline(loader)
  env = RBS::Environment.from_loader(loader)
  force_materialize(env)
  resolved = env.resolve_type_names
  force_materialize(resolved)
end

def measure(label, gc_disabled: false)
  GC.start
  GC.start                   # try to reach a stable baseline
  gc_before = GC.stat
  rss_before = `ps -o rss= -p #{Process.pid}`.to_i  # KiB
  GC.disable if gc_disabled

  loader = build_loader(SIZE)
  t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  one_pipeline(loader)
  t1 = Process.clock_gettime(Process::CLOCK_MONOTONIC)

  GC.enable if gc_disabled
  gc_after = GC.stat
  rss_after = `ps -o rss= -p #{Process.pid}`.to_i

  {
    label: label,
    wall_ms: (t1 - t0) * 1000,
    minor_gc: gc_after[:minor_gc_count] - gc_before[:minor_gc_count],
    major_gc: gc_after[:major_gc_count] - gc_before[:major_gc_count],
    heap_pages: gc_after[:heap_allocated_pages] - gc_before[:heap_allocated_pages],
    total_allocated_objects:
      gc_after[:total_allocated_objects] - gc_before[:total_allocated_objects],
    rss_growth_kib: rss_after - rss_before,
  }
end

puts "size=#{SIZE} repeats=#{REPEATS}"
puts "-" * 80

# Default: GC enabled
default_runs = REPEATS.times.map { measure("default") }
disabled_runs = REPEATS.times.map { measure("gc_disabled", gc_disabled: true) }

def report(rows)
  rows.first.keys.each do |k|
    next if k == :label
    vs = rows.map { |r| r[k] }
    if vs.first.is_a?(Float)
      printf "  %-25s min=%9.2f  avg=%9.2f  max=%9.2f\n",
             k, vs.min, vs.sum / vs.size, vs.max
    else
      printf "  %-25s min=%9d  avg=%9.1f  max=%9d\n",
             k, vs.min, vs.sum.to_f / vs.size, vs.max
    end
  end
end

puts "\n[GC enabled]"
report(default_runs)

puts "\n[GC disabled during pipeline]"
report(disabled_runs)

d_min = default_runs.map { |r| r[:wall_ms] }.min
x_min = disabled_runs.map { |r| r[:wall_ms] }.min
puts "\nWall-time speedup with GC disabled: #{format('%.2fx', d_min / x_min)}"
puts "Time saved: #{format('%.1f ms (%.1f%%)', d_min - x_min, 100.0 * (d_min - x_min) / d_min)}"
