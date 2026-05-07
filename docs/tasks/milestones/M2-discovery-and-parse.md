# M2: File Discovery and Parallel Parse

## Goal

Implement **discovery + parallel parse + Rust-side entry construction** in
`librbs-core`. Nothing is exposed to the Ruby side yet (no `*_decls`
patches). M2 introduces no observable user-facing changes; verification is
done via CI and unit tests only.

## Prerequisites

- M1 is complete (workspace, native loading, CI scaffold).
- `crates/ruby-rbs`'s `parse(&str) -> Result<SignatureNode<'_>, String>` is
  available via the path dependency on `vendor/rbs/rust/ruby-rbs`.
- See [../reference.md](../reference.md), section "Referenced in M2".

## Deliverables

```
crates/librbs-core/src/
├── lib.rs
├── arena.rs
├── interner.rs
├── discovery/
│   ├── mod.rs
│   ├── repository.rs
│   └── walker.rs
├── source.rs
├── env/
│   ├── mod.rs
│   ├── entry.rs
│   ├── insert.rs
│   └── use_map.rs
└── error.rs
```

A single thin native API is added to `librbs-ruby` for testing purposes only.

## Tasks

### 1. TypeName Interner (`interner.rs`)

- `SymbolInterner` issuing `Sym(u32)` (string → Sym).
- `TypeNameInterner` issuing `TypeNameSym(u32)`:
  - Hash-cons over `(parent_namespace_sym, last_segment_sym, kind) → TypeNameSym`
  - `kind` distinguishes class-like / interface / type-alias.
- `with_prefix(prefix_sym, inner_sym) -> TypeNameSym` is one integer op + one
  hash lookup.
- Reverse lookup (Sym → string) backed by a single `Vec<&str>`.

### 2. Arena (`arena.rs`)

A struct holding one or more `bumpalo::Bump`s (per-source). `ruby-rbs`'s
`SignatureNode<'_>` shares its lifetime with the parser, so initially:

- Keep each per-file parser instance alive (an effective
  `Vec<SignatureNode<'static>>`).
- Defer copying parse results into a Rust-side enum (revisit in M3 if
  needed).

The straightforward approach is to **hold each per-file `SignatureNode`
inside `Source`**. Abstract the arena layer as a small façade so the
implementation can be swapped later.

### 3. Repository (`discovery/repository.rs`)

Mirror the behavior of `vendor/rbs/lib/rbs/repository.rb`:

- `Repository` struct: `dirs: Vec<PathBuf>`, `gems: HashMap<String, GemRBS>`.
- `GemRBS`: gem name + paths. `load!` walks `paths/<version>` and collects
  versions that pass a `Gem::Version::correct?` equivalent (semver-compatible
  parse).
- `find_best_version`: bsearch on the sorted list.
- `add(dir)`: each child directory under `dir` becomes a gem entry.
- `lookup(gem, version) -> Option<PathBuf>`.
- By default, `add(vendor/rbs/stdlib/)` (the `DEFAULT_STDLIB_ROOT` equivalent).

We do not need to fully reproduce `Gem::Version` semantics; we only need to
**select the same best_version for the same input**. Reference the
comparison rules of `Gem::Version` `release`.

### 4. File walker (`discovery/walker.rs`)

Equivalent of `FileFinder.each_file` from
`vendor/rbs/lib/rbs/file_finder.rb`:

- Input: directory path + `skip_hidden: bool`.
- Output: `Vec<PathBuf>` (extension `.rbs`, sorted; when `skip_hidden`,
  exclude paths whose any directory component starts with `_`).
- Implementation: the `walkdir` or `ignore` crate. Parallelism happens later
  via rayon, so this function can be sequential.

### 5. EnvironmentLoader (`discovery/mod.rs`)

Port `EnvironmentLoader#each_dir` and `#each_signature`:

- `Loader` struct: `core_root`, `repository`, `libs`, `dirs`.
- `add(path)`, `add_library(name, version)`.
- `each_dir() -> Vec<(SourceTag, PathBuf)>`.
- `discover_files() -> Vec<(SourceTag, PathBuf)>` (with deduplication).

`SourceTag` records core / library / user-dir distinctions, mirroring the
Ruby `source` argument.

### 6. Source (`source.rs`)

The `Source::RBS` equivalent:

```rust
struct Source {
    buffer: Buffer,                   // name + content
    directives: Vec<DirectiveNode>,   // borrowed from ruby-rbs's generated types
    declarations: Vec<DeclNode>,
    parser: ManagedParser,            // owns the lifetime
}
```

`Buffer`: `name: PathBuf`, `content: String`, `line_offsets: Vec<usize>`.
Provide methods analogous to `Buffer#pos_to_loc` / `loc_to_pos`.

### 7. Parallel parse

`rayon::par_iter` over files:

```rust
let sources: Vec<Source> = files
    .into_par_iter()
    .map(|(tag, path)| {
        let content = std::fs::read_to_string(&path)?;
        let parsed = ruby_rbs::node::parse(&content)?;
        Ok(Source::new(tag, path, content, parsed))
    })
    .collect::<Result<Vec<_>, _>>()?;
```

### 8. Entry structures (`env/entry.rs`)

Mirroring `vendor/rbs/lib/rbs/environment/class_entry.rb` and
`module_entry.rb`:

- `ClassEntry { name: TypeNameSym, context_decls: Vec<(ContextSym, DeclRef)> }`
- `ModuleEntry { name: TypeNameSym, context_decls: Vec<(ContextSym, DeclRef)> }`
- `InterfaceEntry`, `TypeAliasEntry`, `ConstantEntry`, `GlobalEntry`,
  `ClassAliasEntry`, `ModuleAliasEntry`.

`DeclRef` is a lightweight reference such as `(source_index, decl_index)`.

`primary_decl` can wait until M3 (when its consumers appear).

### 9. insert (`env/insert.rs`)

Port `insert_rbs_decl` from `vendor/rbs/lib/rbs/environment.rb:277-372`,
including the duplicate detection that produces `DuplicatedDeclarationError`.

Parallelization plan:

- Build **per-source local entry tables** (no contention; safe to parallelize).
- A serial phase merges them into the global table and detects collisions.

Parallelization is optional. Start sequential; parallelize later if
profiling demands it.

### 10. Environment (`env/mod.rs`)

```rust
pub struct Environment {
    pub interner: TypeNameInterner,
    pub sources: Vec<Source>,
    pub class_decls: HashMap<TypeNameSym, ClassLikeEntry>,
    pub interface_decls: HashMap<TypeNameSym, InterfaceEntry>,
    pub type_alias_decls: HashMap<TypeNameSym, TypeAliasEntry>,
    pub constant_decls: HashMap<TypeNameSym, ConstantEntry>,
    pub class_alias_decls: HashMap<TypeNameSym, ClassAliasLikeEntry>,
    pub global_decls: HashMap<GlobalSym, GlobalEntry>,
    pub resolution: Option<Resolution>,   // populated in M3
}

impl Environment {
    pub fn from_loader(loader: &Loader) -> Result<Self, Error>;
}
```

### 11. Native API (minimal)

A single function for sanity-check purposes:

```rust
// added to crates/librbs-ruby/src/lib.rs
fn build_environment_count(core_root: PathBuf) -> Result<usize, Error> {
    let loader = librbs_core::Loader::with_core_root(core_root);
    let env = librbs_core::Environment::from_loader(&loader)?;
    Ok(env.class_decls.len())
}
```

This lets us confirm "class_decls count" from
`Librbs::Native.build_environment_count(path)` without any Ruby
materialization.

## Tests

### Rust-side (`crates/librbs-core/tests/`)

- discovery: rooting at `vendor/rbs/core`, file count matches expectation.
- parse: every file parses (parity with `ruby-rbs/tests/sanity.rs`).
- insert: deterministic entry counts on core+stdlib (rerunning yields the
  same numbers).

### Ruby-side (`spec/unit/`)

```ruby
RSpec.describe "Librbs::Native build_environment_count" do
  it "returns class_decls count for core root" do
    count = Librbs::Native.build_environment_count(
      Pathname(__dir__).join("../../vendor/rbs/core").to_s
    )
    expect(count).to be > 0
  end
end
```

## Acceptance

- [x] `cargo test -p librbs-core` is green.
- [x] core+stdlib loading succeeds on the Rust side, and `class_decls.len()`
      lands in the same order of magnitude as pure RBS
      (`RBS::Environment.from_loader(RBS::EnvironmentLoader.new).class_decls.size`).
      An exact match isn't required yet (M3 will pin it down). A 1% delta is
      acceptable.
- [x] `Librbs::Native.build_environment_count` runs without errors.
- [x] All CI jobs are green.

## Out of scope for this milestone

- Name resolution (M3).
- Patches for Ruby-side `*_decls` (M3).
- Any path that turns `TypeName` into Ruby (M3).
- Implementing the `Resolution` struct (M3).

## Pitfalls and mitigation

### Lifetime of ruby-rbs AST nodes

`SignatureNode<'a>` is tied to the parser, so `Source` must hold both the
parser and the signature in a single struct. To avoid self-referential
struct issues, either `Box::leak` the parser or use `Pin`. The simplest path
is a `ManagedParser` that internally holds a raw pointer.

The `examples/locations.rs` in `ruby-rbs` may demonstrate a working pattern.

### Pathname ↔ PathBuf

Paths come from Ruby as `Pathname`. The native API receives them as `String`
and converts via `PathBuf::from` on the Rust side.

### Encoding

`vendor/rbs/lib/rbs/environment_loader.rb:158` reads with
`path.read(encoding: "UTF-8")`. Rust's `read_to_string` likewise assumes
UTF-8. RBS files are UTF-8 only, so this is fine. If a BOM appears, strip it.

## Next milestone

→ [M3-environment-and-resolver.md](M3-environment-and-resolver.md)
