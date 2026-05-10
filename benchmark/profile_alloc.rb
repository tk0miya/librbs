# frozen_string_literal: true

# Allocation-mode stackprof: counts Ruby objects created per frame /
# class. Combined with the wall-mode profile, this isolates the
# dominant *allocation source* inside materialize, even when most of
# that work happens in the Rust extension via the C API.
#
# Usage:
#   bundle exec ruby benchmark/profile_alloc.rb [size] [iterations]

$LOAD_PATH.unshift(File.expand_path("../lib", __dir__))
require "rbs"
require "librbs"
require "stackprof"
require "objspace"

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

def one_pipeline(loader)
  env = RBS::Environment.from_loader(loader)
  env.class_decls.size
  resolved = env.resolve_type_names
  resolved.class_decls.size
end

# ---------- per-class allocation counts ----------

GC.start
before_counts = ObjectSpace.count_objects_size
ObjectSpace.trace_object_allocations_clear if ObjectSpace.respond_to?(:trace_object_allocations_clear)
GC.start

class_counts = Hash.new(0)
ObjectSpace.trace_object_allocations do
  loader = build_loader(SIZE)
  one_pipeline(loader)
end

# Walk live objects; for each, ask trace where it was allocated and
# bucket by class.
ObjectSpace.each_object do |obj|
  begin
    cls = obj.class
    class_counts[cls] += 1
  rescue
    # Some objects raise on .class; skip.
  end
end

puts "size=#{SIZE}"
puts "\nLive objects after one pipeline (top 25 classes):"
puts "-" * 70
class_counts.sort_by { |_, v| -v }.first(25).each do |k, v|
  printf "  %10d  %s\n", v, k
end

# ---------- stackprof allocation profile (frame-attributed) ----------

dump = File.expand_path("../stackprof_alloc_#{SIZE}.dump", __dir__)
StackProf.run(mode: :object, raw: true, interval: 1, out: dump) do
  ITERS.times do
    loader = build_loader(SIZE)
    one_pipeline(loader)
  end
end

puts "\nWrote #{dump}"

report = StackProf::Report.new(Marshal.load(File.binread(dump)))
puts "\nTop frames by *allocations* (stackprof :object, #{ITERS} iters):"
puts "-" * 70
report.print_text(false, 30)
