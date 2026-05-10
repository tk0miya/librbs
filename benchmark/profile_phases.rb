# frozen_string_literal: true

# Reports per-phase exclusive self-time inside `materialize_all`,
# captured by the Rust-side `phase_timer`. Use to identify which
# materialize sub-pass dominates wall time after the GC-disable patch.
#
# Usage:
#   bundle exec ruby benchmark/profile_phases.rb [size] [iterations]

$LOAD_PATH.unshift(File.expand_path("../lib", __dir__))
require "rbs"
require "librbs"

SIZE = (ARGV[0] || "large").to_sym
ITERS = (ARGV[1] || 10).to_i

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

# Warm-up + reset.
loader = build_loader(SIZE)
env = RBS::Environment.from_loader(loader)
env.class_decls.size
env.resolve_type_names.class_decls.size
Librbs::Native.materialize_phase_dump  # discard warmup

# Actual measurement: run the pipeline ITERS times, then dump.
ITERS.times do
  loader = build_loader(SIZE)
  env = RBS::Environment.from_loader(loader)
  env.class_decls.size
  env.resolve_type_names.class_decls.size
end

puts "size=#{SIZE} iterations=#{ITERS}"
puts Librbs::Native.materialize_phase_dump
