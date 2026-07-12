# frozen_string_literal: true
#
# Times an external command end-to-end (VM boot + `require` + work) over
# RUNS iterations and reports min / median / mean wall clock in
# milliseconds. Used by `list_benchmark.sh` to time the real `rbs list`
# command; see that script for how the pure-RBS and librbs cases are
# constructed. The measured command's own output is discarded — we only
# care about its wall time.
#
#     ruby list_driver.rb <label> <cmd> [args...]

RUNS   = Integer(ENV.fetch("RUNS", "20"))
WARMUP = Integer(ENV.fetch("WARMUP", "3"))
LABEL  = ARGV.shift or abort "usage: list_driver.rb <label> <cmd> [args...]"
CMD    = ARGV

def clock = Process.clock_gettime(Process::CLOCK_MONOTONIC)

def once(cmd)
  t0 = clock
  ok = system(*cmd, out: File::NULL, err: File::NULL)
  t1 = clock
  abort "command failed: #{cmd.inspect}" unless ok
  (t1 - t0) * 1000.0
end

WARMUP.times { once(CMD) }
samples = Array.new(RUNS) { once(CMD) }.sort

pct = ->(sorted, p) { sorted[(sorted.length * p).floor.clamp(0, sorted.length - 1)] }

min  = samples.first
med  = pct.call(samples, 0.5)
mean = samples.sum / samples.length

puts format("%-14s runs=%2d  min=%.1f  median=%.1f  mean=%.1f  (ms)",
            LABEL, RUNS, min, med, mean)
