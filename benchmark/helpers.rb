# frozen_string_literal: true

# Shared infrastructure for the benchmark suite.
#
# The pure-RBS and librbs cases cannot coexist in one process — `require
# "librbs"` monkey-patches `RBS::Environment` globally, so once it loads
# there is no clean way back. Each measurement therefore happens in a
# subprocess that requires exactly one of the two implementations.
#
# Run benchmarks via `bundle exec`, e.g.:
#
#     bundle exec ruby benchmark/benchmark.rb
#
# `bundle exec` propagates RUBYOPT to children so the gem environment is
# consistent between parent and child processes.

require "open3"
require "rbconfig"

module BenchHelpers
  ROOT = File.expand_path("..", __dir__)

  # The bench workload loads core + the `rbs_collection.lock.yaml` of a
  # real-world OSS project (kaigionrails/conference-app). The project is
  # cloned into `fixtures/conference-app/` (never vendored — see
  # `../README.md` for the pinned clone + collection-install steps).
  # That one-shot populates both the collection cache and the gem path
  # (some sigs are `type: rubygems` and ship inside the gem itself).
  COLLECTION_LOCKFILE = "fixtures/conference-app/rbs_collection.lock.yaml"

  IMPLS = %i[pure_rbs librbs].freeze

  module_function

  # Returns an env hash safe to pass to Open3 from inside `bundle exec`.
  # Bundler.unbundled_env strips bundler keys from a copy of ENV, but
  # Open3.capture3(env, ...) only overrides keys present in `env` — keys
  # absent from the hash are inherited from the parent process, so
  # BUNDLE_GEMFILE / RUBYOPT etc. leak through and the child sees the
  # parent's restricted Gemfile gem set instead of the full local gem
  # environment. Explicitly setting those keys to nil tells Open3 to
  # remove them in the child.
  def unbundled_env
    base =
      if defined?(Bundler)
        Bundler.unbundled_env
      else
        ENV.to_h
      end
    base = base.dup
    ENV.each_key do |k|
      next if base.key?(k)
      base[k] = nil
    end
    base
  end

  def loader_setup
    lock_abs = File.expand_path(COLLECTION_LOCKFILE, __dir__)
    cache_dir = File.join(File.dirname(lock_abs), ".gem_rbs_collection")
    <<~RUBY
      loader = RBS::EnvironmentLoader.new
      require "yaml"
      _lock_path = Pathname(#{lock_abs.inspect})
      unless File.directory?(#{cache_dir.inspect})
        abort "[bench] collection cache missing at #{cache_dir} -- run the " \\
              "conference-app clone + `rbs collection install` steps in " \\
              "benchmark/README.md"
      end
      _lockfile = RBS::Collection::Config::Lockfile.from_lockfile(
        lockfile_path: _lock_path,
        data: YAML.load_file(_lock_path)
      )
      loader.add_collection(_lockfile)
    RUBY
  end

  def requires_for(impl)
    case impl
    when :pure_rbs
      <<~RUBY
        require "rbs"
      RUBY
    when :librbs
      <<~RUBY
        $LOAD_PATH.unshift(File.expand_path("lib", #{ROOT.inspect}))
        require "rbs"
        require "librbs"
        unless defined?(Librbs::Native::EnvironmentLoader)
          abort "[bench] librbs native extension is not loaded; run `bundle exec rake compile` first"
        end
      RUBY
    else
      raise ArgumentError, "unknown impl: #{impl.inspect}"
    end
  end

  # Runs `body` in a fresh `ruby -e` subprocess with the given impl loaded.
  # Returns the subprocess stdout as a string.
  #
  # Bundler env (BUNDLE_GEMFILE / RUBYOPT / GEM_PATH) is stripped so the
  # child sees every locally installed gem, not just the parent's
  # restricted Gemfile set. Without this, gems pulled in by the
  # collection (e.g. webrick) fail to resolve under `bundle exec`.
  def run_subprocess(impl:, body:)
    code = +""
    code << requires_for(impl)
    code << "\n"
    code << body
    env = unbundled_env
    out, err, status = Open3.capture3(env, RbConfig.ruby, "-e", code, chdir: ROOT)
    unless status.success?
      raise <<~MSG
        bench subprocess failed (impl=#{impl})
        --- stdout ---
        #{out}
        --- stderr ---
        #{err}
      MSG
    end
    out
  end

  # Measures the wall time of `expr` in a fresh subprocess. The loader is
  # built fresh inside the timed block on each repeat so we measure cold-
  # start cost (parse + construct), which is what loader code actually pays
  # in real Steep usage.
  #
  # Returns the minimum of `repeats` runs (least noisy estimate).
  def measure_realtime(impl:, expr:, repeats: 3)
    body = <<~RUBY
      require "benchmark"
      times = []
      #{repeats}.times do
        #{loader_setup}
        GC.start
        times << Benchmark.realtime do
          #{expr}
        end
      end
      puts times.min
    RUBY
    run_subprocess(impl: impl, body: body).strip.to_f
  end

  # Measures iterations-per-second via `benchmark-ips` in a fresh subprocess.
  # Useful for steady-state comparison; cold-start is better captured by
  # `measure_realtime` above.
  #
  # Returns ips (Float).
  def measure_ips(impl:, expr:, warmup: 1, time: 3)
    body = <<~RUBY
      require "benchmark/ips"
      job = Benchmark::IPS::Job.new
      job.config(warmup: #{warmup}, time: #{time}, quiet: true)
      job.item("bench") do
        #{loader_setup}
        #{expr}
      end
      job.run
      puts job.entries.first.stats.central_tendency
    RUBY
    run_subprocess(impl: impl, body: body).strip.to_f
  end

  def format_ms(seconds)
    format("%.1f ms", seconds * 1000)
  end

  def format_speedup(pure, lib)
    return "n/a" if lib.zero?
    format("%.2fx", pure / lib)
  end

  # Drives a single benchmark across {pure_rbs, librbs} and prints a
  # Markdown table. `expr` is the workload string evaluated inside the
  # timed block; it has access to the local `loader` from `loader_setup`.
  def report_realtime(title:, expr:, repeats: 3)
    puts "## #{title}"
    puts
    pure = measure_realtime(impl: :pure_rbs, expr: expr, repeats: repeats)
    lib  = measure_realtime(impl: :librbs,   expr: expr, repeats: repeats)
    rows = [[format_ms(pure), format_ms(lib), format_speedup(pure, lib)]]
    print_table(["pure RBS", "librbs", "speedup"], rows)
    puts
  end

  def print_table(headers, rows)
    widths = headers.each_with_index.map do |h, i|
      ([h.length] + rows.map { |r| r[i].to_s.length }).max
    end
    fmt = ->(cells) {
      "| " + cells.each_with_index.map { |c, i| c.to_s.ljust(widths[i]) }.join(" | ") + " |"
    }
    puts fmt.call(headers)
    puts "|" + widths.map { |w| "-" * (w + 2) }.join("|") + "|"
    rows.each { |r| puts fmt.call(r) }
  end
end
