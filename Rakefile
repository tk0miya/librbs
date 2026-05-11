# frozen_string_literal: true

require "bundler/gem_tasks"
require "rspec/core/rake_task"
require "rake/testtask"

RSpec::Core::RakeTask.new(:spec)

# Upstream environment / loader / walker tests copied verbatim from
# `vendor/rbs/test/rbs/`, run under `require "librbs"` so the patched
# code paths are exercised. See `test/test_helper.rb` for the harness.
Rake::TestTask.new(:test) do |t|
  t.libs << "test"
  t.test_files = FileList["test/rbs/**/*_test.rb"]
  t.verbose = true
end

require "rb_sys/extensiontask"

task build: :compile

GEMSPEC = Gem::Specification.load("librbs.gemspec")

RbSys::ExtensionTask.new("librbs", GEMSPEC) do |ext|
  ext.lib_dir = "lib/librbs"
end

task default: %i[compile spec test]
