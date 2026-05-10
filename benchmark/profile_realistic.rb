# frozen_string_literal: true

# Realistic single-materialize pipeline: load + resolve + one
# materialize. Mirrors how Steep actually drives the gem (no
# pre-resolve `class_decls` peek).
#
#   bundle exec ruby benchmark/profile_realistic.rb [size] [repeats]

$LOAD_PATH.unshift(File.expand_path("../lib", __dir__))
require "rbs"
require "librbs"

SIZE = (ARGV[0] || "large").to_sym
REPEATS = (ARGV[1] || 7).to_i

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

def measure(size)
  loader = build_loader(size)
  GC.start
  t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  env = RBS::Environment.from_loader(loader)
  t1 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  resolved = env.resolve_type_names
  t2 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  resolved.class_decls.size  # triggers single materialize
  t3 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  {
    from_loader: (t1 - t0) * 1000,
    resolve:     (t2 - t1) * 1000,
    materialize: (t3 - t2) * 1000,
    total:       (t3 - t0) * 1000,
  }
end

# Warm up
measure(SIZE)
runs = REPEATS.times.map { measure(SIZE) }

puts "size=#{SIZE} repeats=#{REPEATS} (min)"
puts "-" * 50
%i[from_loader resolve materialize total].each do |k|
  vs = runs.map { |r| r[k] }
  printf "  %-12s %8.2f ms\n", k, vs.min
end

# Phase breakdown
Librbs::Native.materialize_phase_dump  # discard
10.times { measure(SIZE) }
puts
puts Librbs::Native.materialize_phase_dump
