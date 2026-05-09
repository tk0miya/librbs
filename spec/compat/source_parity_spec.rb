# frozen_string_literal: true

require "tmpdir"
require "rbs"

require_relative "../support/without_librbs"

# M3k: `RBS::Environment#sources` / `#declarations` parity with pure
# RBS. A small inline fixture is dumped both via the librbs-patched
# env and via a fresh pure-RBS subprocess; the per-source declarations
# count and a stable shape signature are expected to agree.
RSpec.describe "Environment#sources parity" do
  let(:fixtures) do
    {
      "single_class" => "class Foo\nend\n",
      "module_with_nested" => <<~RBS,
        module Outer
          class Inner end
          type t = Integer
        end
      RBS
      "with_use_directives" => <<~RBS,
        use Foo::Bar
        use ::Baz::*

        class Quux end
      RBS
      "constant_and_global" => <<~RBS
        Pi: Float
        $logger: Integer
      RBS
    }
  end

  it "matches librbs and pure-RBS for sources count and per-source declarations count" do
    fixtures.each do |label, content|
      Dir.mktmpdir do |dir|
        File.write(File.join(dir, "#{label}.rbs"), content)
        loader = RBS::EnvironmentLoader.new(core_root: nil)
        loader.add(path: Pathname(dir))
        env = RBS::Environment.from_loader(loader)
        librbs_signature = source_signature(env)

        pure_signature = without_librbs(<<~RUBY)
          require "rbs"
          loader = RBS::EnvironmentLoader.new(core_root: nil)
          loader.add(path: Pathname(#{dir.inspect}))
          env = RBS::Environment.from_loader(loader)
          puts env.sources.size
          env.sources.each do |s|
            puts s.declarations.size
          end
        RUBY

        expect(librbs_signature).to eq(pure_signature), "mismatch for fixture #{label}"
      end
    end
  end

  def source_signature(env)
    out = +""
    out << "#{env.sources.size}\n"
    env.sources.each do |s|
      out << "#{s.declarations.size}\n"
    end
    out
  end
end
