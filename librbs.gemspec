# frozen_string_literal: true

require_relative "lib/librbs/version"

Gem::Specification.new do |spec|
  spec.name = "librbs"
  spec.version = Librbs::VERSION
  spec.authors = ["Claude"]
  spec.email = ["noreply@anthropic.com"]

  spec.summary = "Rust-backed accelerator for the RBS loader."
  spec.description = "Speeds up RBS::Environment.from_loader and resolve_type_names by " \
                     "monkey-patching RBS with a Rust implementation. Drop-in: " \
                     'just `require "librbs"`.'
  spec.homepage = "https://github.com/tk0miya/librbs"
  spec.required_ruby_version = ">= 3.3"
  spec.platform = Gem::Platform::RUBY

  spec.metadata["homepage_uri"] = spec.homepage
  spec.metadata["source_code_uri"] = spec.homepage

  spec.files = Dir[
    "lib/**/*",
    "ext/**/*",
    "crates/**/*",
    "Cargo.toml",
    "Cargo.lock",
    "README.md"
  ].reject do |f|
    (f.start_with?("crates/") && f.include?("/target/")) ||
      /\.(so|bundle|dylib|dll|o|a)$/.match?(f)
  end

  spec.bindir = "exe"
  spec.executables = []
  spec.require_paths = ["lib"]
  spec.extensions = ["ext/librbs/extconf.rb"]

  spec.add_dependency "rbs", "~> 4.0"
  spec.add_dependency "rb_sys", "~> 0.9"
end
