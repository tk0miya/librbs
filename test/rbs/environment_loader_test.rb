require "test_helper"

class RBS::EnvironmentLoaderTest < Test::Unit::TestCase
  include TestHelper

  Environment = RBS::Environment
  EnvironmentLoader = RBS::EnvironmentLoader
  Declarations = RBS::AST::Declarations
  TypeName = RBS::TypeName
  Namespace = RBS::Namespace

  def mktmpdir
    Dir.mktmpdir do |path|
      yield Pathname(path)
    end
  end

  def write_signatures(path:)
    path.join("models").mkdir
    path.join("models/person.rbs").write(<<-RBS)
class Person
end
    RBS

    path.join("controllers").mkdir
    path.join("controllers/people_controller.rbs").write(<<-RBS)
class PeopleController
end
    RBS

    path.join("_private").mkdir
    path.join("_private/person.rbs").write(<<-RBS)
class Person::Internal
end
    RBS
  end

  def test_loading_empty
    loader = EnvironmentLoader.new

    env = Environment.new
    loader.load(env: env)

    # librbs adjusted: upstream verifies stringio injection via the
    # [[decl, path, source], ...] return value of `load`. The librbs
    # patch returns `[]` (lazy materialisation boundary), so the
    # stringio-was-injected invariant is checked through
    # `loader.libs` instead. Upstream's second assertion
    # (`loaded.all? path_type == :core`) had no librbs-side
    # equivalent — the `:core` source tag is not retained — and is
    # dropped rather than substituted with a divergent check.
    assert loader.libs.any? { |lib| lib.name == "stringio" }
  end

  def test_loading_no_core
    loader = EnvironmentLoader.new(core_root: nil)

    env = Environment.new()
    loader.load(env: env)

    # librbs adjusted: upstream asserts `load`'s return value is empty.
    # The librbs patch returns `[]` unconditionally, so that
    # assertion is dropped. The test remains as a smoke check that
    # `core_root: nil` does not raise.
  end

  def test_loading_dir
    mktmpdir do |path|
      write_signatures(path: path)

      loader = EnvironmentLoader.new
      loader.add(path: path)

      env = Environment.new
      loader.load(env: env)

      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::Person")
      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::PeopleController")
      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::Person::Internal")
    end
  end

  def test_loading_stdlib
    mktmpdir do |path|
      loader = EnvironmentLoader.new
      loader.add(library: "uri")

      env = Environment.new
      loader.load(env: env)

      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::URI")
    end
  end

  def test_loading_library_from_gem_repo
    mktmpdir do |path|
      (path + "gems").mkdir
      (path + "gems/gem1").mkdir
      (path + "gems/gem1/1.2.3").mkdir

      write_signatures(path: path + "gems/gem1/1.2.3")

      repo = RBS::Repository.new()
      repo.add(path + "gems")

      loader = EnvironmentLoader.new(repository: repo)
      loader.add(library: "gem1", version: "1.2.3")

      env = Environment.new
      loader.load(env: env)

      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::Person")
      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::PeopleController")
      refute_operator env.class_decls, :key?, RBS::TypeName.parse("::Person::Internal")
    end
  end

  def test_loading_unknown_library
    repo = RBS::Repository.new()

    loader = EnvironmentLoader.new(repository: repo)
    loader.add(library: "gem1", version: "1.2.3")

    env = Environment.new

    assert_raises EnvironmentLoader::UnknownLibraryError do
      loader.load(env: env)
    end
  end

  def test_loading_twice
    mktmpdir do |path|
      write_signatures(path: path)

      loader = EnvironmentLoader.new
      loader.add(path: path)
      loader.add(path: path + "models")

      env = Environment.new
      loader.load(env: env)

      # librbs adjusted: upstream counts `Person` occurrences in the
      # `[[decl, path, source], ...]` array returned by `load`. The
      # librbs patch returns `[]`, so that assertion is dropped.
      # The test remains as a smoke check that overlapping
      # `add(path:)` calls do not raise.
    end
  end

  def test_loading_from_gem
    omit "Test gem `rbs-amber` is unavailable" unless has_gem?("rbs-amber")

    mktmpdir do |path|
      repo = RBS::Repository.new()

      loader = EnvironmentLoader.new(repository: repo)
      loader.add(library: "rbs-amber", version: nil)

      env = Environment.new
      loader.load(env: env)

      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::Amber")
    end
  end

  def test_loading_from_gem_without_rbs
    omit if skip_minitest?

    mktmpdir do |path|
      repo = RBS::Repository.new()

      loader = EnvironmentLoader.new(repository: repo)
      loader.add(library: "non_existent_gems", version: nil)

      env = Environment.new

      assert_raises EnvironmentLoader::UnknownLibraryError do
        loader.load(env: env)
      end
    end
  end

  def test_loading_dependencies
    mktmpdir do |path|
      loader = EnvironmentLoader.new
      loader.add(library: "psych")

      env = Environment.new
      loader.load(env: env)

      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::Psych")
      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::DBM")
      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::PStore")
    end
  end

  def test_loading_from_rbs_collection
    mktmpdir do |path|
      lockfile_path = path.join('rbs_collection.lock.yaml')
      lockfile_path.write(<<~YAML)
        sources:
          - name: ruby/gem_rbs_collection
            remote: https://github.com/ruby/gem_rbs_collection.git
            revision: b4d3b346d9657543099a35a1fd20347e75b8c523
            repo_dir: gems
        path: '.gem_rbs_collection'
        gems:
          - name: ast
            version: "2.4"
            source:
              name: ruby/gem_rbs_collection
              remote: https://github.com/ruby/gem_rbs_collection.git
              revision: b4d3b346d9657543099a35a1fd20347e75b8c523
              repo_dir: gems
              type: git
          - name: rainbow
            version: "3.0"
            source:
              name: ruby/gem_rbs_collection
              remote: https://github.com/ruby/gem_rbs_collection.git
              revision: b4d3b346d9657543099a35a1fd20347e75b8c523
              repo_dir: gems
              type: git
      YAML
      RBS::Collection::Installer.new(lockfile_path: lockfile_path, stdout: StringIO.new).install_from_lockfile
      lock = RBS::Collection::Config::Lockfile.from_lockfile(lockfile_path: lockfile_path, data: YAML.load_file(lockfile_path))

      repo = RBS::Repository.new()

      loader = EnvironmentLoader.new(repository: repo)
      loader.add_collection(lock)

      env = Environment.new
      loader.load(env: env)

      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::AST")
      assert_operator env.class_decls, :key?, RBS::TypeName.parse("::Rainbow")
      assert repo.dirs.include? lock.fullpath
    end
  end

  def test_loading_from_rbs_collection__gem_version_mismatch
    omit "Test gem `rbs-amber` is unavailable" unless has_gem?("rbs-amber")

    mktmpdir do |path|
      lockfile_path = path.join('rbs_collection.lock.yaml')
      lockfile_path.write(<<~YAML)
        sources:
          - name: ruby/gem_rbs_collection
            remote: https://github.com/ruby/gem_rbs_collection.git
            revision: b4d3b346d9657543099a35a1fd20347e75b8c523
            repo_dir: gems
        path: '.gem_rbs_collection'
        gems:
          - name: rbs-amber
            version: "1.1"
            source:
              type: "rubygems"
      YAML
      RBS::Collection::Installer.new(lockfile_path: lockfile_path, stdout: StringIO.new).install_from_lockfile
      lock = RBS::Collection::Config::Lockfile.from_lockfile(lockfile_path: lockfile_path, data: YAML.load_file(lockfile_path))

      repo = RBS::Repository.new()

      loader = EnvironmentLoader.new(repository: repo)

      io = StringIO.new
      old_output = RBS.logger_output
      RBS.logger_output = io
      begin
        loader.add_collection(lock)
        env = Environment.new
        loader.load(env: env)
      ensure
        RBS.logger_output = old_output
      end

      assert_operator(
        io.string,
        :include?,
        "Loading type definition from gem `rbs-amber-1.0.0` because locked version `1.1` is unavailable. Try `rbs collection update` to fix the (potential) issue."
      )
    end
  end

  def test_loading_from_rbs_collection_git_source_without_install
    mktmpdir do |path|
      lockfile_path = path.join('rbs_collection.lock.yaml')
      lockfile_path.write(<<~YAML)
        sources:
          - name: ruby/gem_rbs_collection
            remote: https://github.com/ruby/gem_rbs_collection.git
            revision: b4d3b346d9657543099a35a1fd20347e75b8c523
            repo_dir: gems
        path: '.gem_rbs_collection'
        gems:
          - name: ast
            version: "2.4"
            source:
              name: ruby/gem_rbs_collection
              remote: https://github.com/ruby/gem_rbs_collection.git
              revision: b4d3b346d9657543099a35a1fd20347e75b8c523
              repo_dir: gems
              type: git
          - name: rainbow
            version: "3.0"
            source:
              name: ruby/gem_rbs_collection
              remote: https://github.com/ruby/gem_rbs_collection.git
              revision: b4d3b346d9657543099a35a1fd20347e75b8c523
              repo_dir: gems
              type: git
      YAML
      lock = RBS::Collection::Config::Lockfile.from_lockfile(lockfile_path: lockfile_path, data: YAML.load_file(lockfile_path.to_s))

      repo = RBS::Repository.new()

      loader = EnvironmentLoader.new(repository: repo)

      assert_raises RBS::Collection::Config::CollectionNotAvailable do
        loader.add_collection(lock)
      end
    end
  end

  def test_loading_from_rbs_collection_local_source_without_install
    mktmpdir do |path|
      lockfile_path = path.join('rbs_collection.lock.yaml')
      lockfile_path.write(<<~YAML)
        sources:
          - type: local
            name: the local source
            path: path/to/local/source
        path: '.gem_rbs_collection'
        gems:
          - name: ast
            version: "2.4"
            source:
              type: local
              name: the local source
              path: path/to/local/source
      YAML
      lock = RBS::Collection::Config::Lockfile.from_lockfile(lockfile_path: lockfile_path, data: YAML.load_file(lockfile_path.to_s))

      repo = RBS::Repository.new()

      loader = EnvironmentLoader.new(repository: repo)

      assert_raises RBS::Collection::Config::CollectionNotAvailable do
        loader.add_collection(lock)
      end
    end
  end
end
