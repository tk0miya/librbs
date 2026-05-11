# frozen_string_literal: true

# Test harness for the environment / loader / walker tests copied verbatim
# from `vendor/rbs/test/rbs/`. The point of running them here (rather than
# under upstream RBS's own test runner) is to exercise the librbs patches:
# `require "librbs"` rewires `RBS::Environment.from_loader`,
# `RBS::Environment#resolve_type_names`, and the materialisation
# accessors, so any divergence from upstream surfaces as a test failure.
#
# This file is intentionally a slimmed-down port of
# `vendor/rbs/test/test_helper.rb` — only the pieces the three copied
# test files actually use are kept (the `ArgumentChecker` /
# `assert_sampling_check` helpers and the `amber` extension are
# dropped). The `SignatureManager` and `TestHelper` surfaces are
# byte-identical to upstream so the copied tests need no edits.

$LOAD_PATH.unshift(File.expand_path(__dir__))

require "tmpdir"
require "stringio"
require "open3"
require "bundler"

require "librbs"
require "test_skip"

unless ENV["XDG_CACHE_HOME"]
  tmpdir = Dir.mktmpdir("librbs-test-")
  ENV["XDG_CACHE_HOME"] = tmpdir

  at_exit do
    FileUtils.rmtree(tmpdir)
    ENV.delete("XDG_CACHE_HOME")
  end
end

require "test/unit"

class Test::Unit::TestCase
  prepend TestSkip
end

module TestHelper
  def has_gem?(*gems)
    gems.each do |gem|
      Gem::Specification.find_by_name(gem)
    end

    true
  rescue Gem::MissingSpecError
    false
  end

  def skip_minitest?
    ENV.key?("NO_MINITEST")
  end

  def parse_type(string, variables: [])
    RBS::Parser.parse_type(string, variables: variables)
  end

  def parse_method_type(string, variables: [])
    RBS::Parser.parse_method_type(string, variables: variables)
  end

  def type_name(string)
    RBS::Namespace.parse(string).yield_self do |namespace|
      last = namespace.path.last
      RBS::TypeName.new(name: last, namespace: namespace.parent)
    end
  end

  def silence_warnings
    klass = RBS.logger.class
    original_method = klass.instance_method(:warn)

    klass.remove_method(:warn)
    klass.define_method(:warn) do |*args, &block|
      block&.call
    end

    yield
  ensure
    klass.remove_method(:warn)
    klass.define_method(:warn, original_method)
  end

  class SignatureManager
    attr_reader :files
    attr_reader :ruby_files
    attr_reader :system_builtin

    def initialize(system_builtin: false)
      @files = {}
      @ruby_files = {}
      @system_builtin = system_builtin

      files[Pathname("builtin.rbs")] = BUILTINS unless system_builtin
    end

    def self.new(**kw)
      instance = super(**kw)

      if block_given?
        yield instance
      else
        instance
      end
    end

    BUILTINS = <<~SIG
      class BasicObject
        def __id__: -> Integer

        private
        def initialize: -> void
      end

      class Object < BasicObject
        include Kernel

        public
        def __id__: -> Integer

        def to_i: -> Integer

        private
        def respond_to_missing?: (Symbol, bool) -> bool
      end

      module Kernel : BasicObject
        private
        def puts: (*untyped) -> nil
      end

      class Class < Module
        def new: (*untyped, **untyped) ?{ (*untyped, **untyped) -> untyped } -> untyped
      end

      class Module
      end

      class String
        include Comparable

        def self.try_convert: (untyped) -> String?
      end

      class Integer
      end

      class Symbol
      end

      module Comparable
      end

      module Enumerable[A]
      end

      class Hash[unchecked out K, unchecked out V]
        include Enumerable[[K, V]]
      end

      class Struct[Elem]
        include Enumerable[Elem?]
      end
    SIG

    def add_file(path, content)
      files[Pathname(path)] = content
    end

    def add_ruby_file(path, content)
      ruby_files[Pathname(path)] = content
    end

    def build
      Dir.mktmpdir do |tmpdir|
        tmppath = Pathname(tmpdir)

        files.each do |path, content|
          absolute_path = tmppath + path
          absolute_path.parent.mkpath
          absolute_path.write(content)
        end

        root =
          if system_builtin
            RBS::EnvironmentLoader::DEFAULT_CORE_ROOT
          else
            nil
          end

        loader = RBS::EnvironmentLoader.new(core_root: root)
        loader.add(path: tmppath)

        env = RBS::Environment.from_loader(loader)

        ruby_files.each do |path, content|
          buffer = RBS::Buffer.new(name: path, content: content)
          prism = Prism.parse(content)
          result = RBS::InlineParser.parse(buffer, prism)
          source = RBS::Source::Ruby.new(buffer, prism, result.declarations, result.diagnostics)
          env.add_source(source)
        end

        env = env.resolve_type_names

        yield env, tmppath
      end
    end
  end

  def assert_any(collection, size: nil)
    assert_any!(collection, size: size) do |item|
      assert yield(item)
    end
  end

  def assert_any!(collection, size: nil)
    assert_equal size, collection.size if size

    *items, last = collection

    if last
      items.each do |item|
        begin
          yield item
        rescue Test::Unit::AssertionFailedError
          next
        else
          # Pass test
          return
        end
      end

      yield last
    else
      assert_block("assert_any! cannot hold for empty collection") { false }
    end
  end

  def assert_write(decls, string)
    writer = RBS::Writer.new(out: StringIO.new)
    writer.write(decls)

    assert_equal string, writer.out.string

    # Check syntax error
    RBS::Parser.parse_signature(writer.out.string)
  end
end
