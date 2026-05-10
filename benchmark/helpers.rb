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
#     bundle exec ruby benchmark/load_and_resolve.rb
#
# `bundle exec` propagates RUBYOPT to children so the gem environment is
# consistent between parent and child processes.

require "open3"
require "rbconfig"

module BenchHelpers
  ROOT = File.expand_path("..", __dir__)

  # Library / collection lists for each measurement size.
  # `EnvironmentLoader.new` already bundles the core signatures, so the
  # small case adds nothing on top.
  #
  # `:large` references an `rbs_collection.lock.yaml` vendored from a
  # real-world OSS project (SeleniumHQ/selenium). Before running the
  # large case, populate the collection cache once with:
  #
  #     cd benchmark/fixtures && \
  #       bundle exec rbs collection install \
  #         --collection selenium.rbs_collection.yaml --frozen
  SIZES = {
    small: {
      libraries: []
    },
    medium: {
      libraries: %w[
        set pathname date time uri optparse logger stringio strscan
      ]
    },
    large: {
      collection: "fixtures/selenium.rbs_collection.lock.yaml"
    }
  }.freeze

  IMPLS = %i[pure_rbs librbs].freeze

  module_function

  def loader_setup(size)
    spec = SIZES.fetch(size)
    lines = ["loader = RBS::EnvironmentLoader.new"]
    Array(spec[:libraries]).each { |l| lines << "loader.add(library: #{l.inspect})" }
    if (collection_rel = spec[:collection])
      lock_abs = File.expand_path(collection_rel, __dir__)
      cache_dir = File.join(File.dirname(lock_abs), ".gem_rbs_collection")
      lines << <<~RUBY
        require "yaml"
        _lock_path = Pathname(#{lock_abs.inspect})
        unless File.directory?(#{cache_dir.inspect})
          abort "[bench] collection cache missing at #{cache_dir} — run: " \\
                "cd benchmark/fixtures && bundle exec rbs collection install " \\
                "--collection selenium.rbs_collection.yaml --frozen"
        end
        _lockfile = RBS::Collection::Config::Lockfile.from_lockfile(
          lockfile_path: _lock_path,
          data: YAML.load_file(_lock_path)
        )
        loader.add_collection(_lockfile)
      RUBY
    end
    lines.join("\n")
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
        unless defined?(Librbs::Native) && Librbs::Native.respond_to?(:build_environment)
          abort "[bench] librbs native extension is not loaded; run `bundle exec rake compile` first"
        end
      RUBY
    else
      raise ArgumentError, "unknown impl: #{impl.inspect}"
    end
  end

  # Runs `body` in a fresh `ruby -e` subprocess with the given impl loaded.
  # Returns the subprocess stdout as a string.
  def run_subprocess(impl:, body:)
    code = +""
    code << requires_for(impl)
    code << "\n"
    code << body
    out, err, status = Open3.capture3(RbConfig.ruby, "-e", code, chdir: ROOT)
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
  def measure_realtime(impl:, size:, expr:, repeats: 3)
    body = <<~RUBY
      require "benchmark"
      times = []
      #{repeats}.times do
        #{loader_setup(size)}
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
  def measure_ips(impl:, size:, expr:, warmup: 1, time: 3)
    body = <<~RUBY
      require "benchmark/ips"
      job = Benchmark::IPS::Job.new
      job.config(warmup: #{warmup}, time: #{time}, quiet: true)
      job.item("bench") do
        #{loader_setup(size)}
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

  # Drives a single benchmark across all SIZES × {pure_rbs, librbs} and
  # prints a Markdown table. `expr` is the workload string evaluated inside
  # the timed block; it has access to the local `loader` from
  # `loader_setup`.
  def report_realtime(title:, expr:, repeats: 3, sizes: SIZES.keys)
    puts "## #{title}"
    puts
    rows = sizes.map do |size|
      pure = measure_realtime(impl: :pure_rbs, size: size, expr: expr, repeats: repeats)
      lib  = measure_realtime(impl: :librbs,   size: size, expr: expr, repeats: repeats)
      [size.to_s, format_ms(pure), format_ms(lib), format_speedup(pure, lib)]
    end
    print_table(["size", "pure RBS", "librbs", "speedup"], rows)
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
