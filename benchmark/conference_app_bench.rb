# frozen_string_literal: true

# Compares pure RBS vs librbs on the full kaigionrails/conference-app
# workload:
#
#   * rbs_collection.lock.yaml (external gems, ~92 sigs)
#   * conference_app_sig/       (the app's own sigs, ~161 .rbs files)
#
# The measured region is the load + resolve pipeline (and, on librbs,
# materialize) — exactly what `rbs -Isig list` runs before its print
# loop:
#
#     env = RBS::Environment.from_loader(loader).resolve_type_names
#     env.class_decls.size   # forces librbs's one-shot materialize_all
#
# The `class_decls.size` line is what makes this comparable across
# implementations: on librbs `from_loader` returns without fully
# materializing every declaration, and `class_decls` is the entry
# point that triggers `Native.materialize_all`.
#
#     bundle exec ruby benchmark/conference_app_bench.rb
#
# See benchmark/README.md for the one-shot fixture setup.

require_relative "helpers"

APP_SIG = File.expand_path("fixtures/conference_app_sig", __dir__)

unless File.directory?(APP_SIG)
  abort <<~MSG
    [bench] conference-app sig snapshot missing at:
      #{APP_SIG}

    Populate it once:
      git clone --depth 1 https://github.com/kaigionrails/conference-app.git \\
        /tmp/conference-app
      cp -r /tmp/conference-app/sig #{APP_SIG}
  MSG
end

BenchHelpers.singleton_class.prepend(Module.new do
  def loader_setup
    base = super
    base + <<~RUBY
      loader.add(path: Pathname(#{APP_SIG.inspect}))
    RUBY
  end
end)

EXPR = <<~RUBY
  env = RBS::Environment.from_loader(loader).resolve_type_names
  env.class_decls.size
RUBY

BenchHelpers.report_realtime(
  title: "conference-app: load + resolve (+ materialize on librbs)",
  expr: EXPR
)
