# vendor/rbs Reference

Code locations in the upstream RBS source to consult during each milestone.
All paths are relative to `vendor/rbs/`. The pinned RBS version is v4.0.2
(`vendor/rbs/rust/rbs_version`).

## Existing Rust assets

| Path | Contents |
|---|---|
| `rust/Cargo.toml` | Workspace declaration (`ruby-rbs`, `ruby-rbs-sys`) |
| `rust/ruby-rbs-sys/` | C parser FFI. `bindgen` generates from `include/rbs/*.h`; `cc` builds `src/**/*.c` |
| `rust/ruby-rbs-sys/build.rs` | bindgen configuration |
| `rust/ruby-rbs-sys/wrapper.h` | bindgen header |
| `rust/ruby-rbs/src/node/mod.rs` | Generated Rust AST plus `parse(&str)` |
| `rust/ruby-rbs/build.rs` | Generates AST types from `config.yml` |
| `rust/ruby-rbs/tests/sanity.rs` | Confirms every RBS file in core+stdlib parses |
| `rust/rbs_version` | Pinned RBS version (v4.0.2) |

We **depend on these crates via Cargo path dependencies** from
`crates/librbs-core` and `crates/librbs-ruby`. We do not copy or fork them.

## C parser internals

| Path | Contents |
|---|---|
| `include/rbs.h`, `include/rbs/*.h` | Public headers |
| `src/parser.c`, `src/lexer.c`, `src/lexer.re` | Parser and lexer |
| `src/ast.c` | AST node manipulation |
| `src/string.c`, `src/util/` | Helpers |
| `ext/rbs_extension/ast_translation.c` | C AST → Ruby AST translation (**reference for M3/M4**) |
| `ext/rbs_extension/main.c` | Ruby extension entry point |

## Ruby implementation under analysis

These are the Ruby files we port to Rust per milestone.

### Referenced in M2

| Path | Lines | Contents |
|---|---|---|
| `lib/rbs/environment_loader.rb` | 1-167 | The full `EnvironmentLoader`. Focus on `each_dir`, `each_signature`, `load` |
| `lib/rbs/file_finder.rb` | 1-28 | `FileFinder.each_file`. Uses `Pathname.glob("**/*.rbs")` and skips hidden dirs |
| `lib/rbs/repository.rb` | 1-127 | `Repository`, `GemRBS`, `VersionPath`. Maps gem name → version → directory |
| `lib/rbs/buffer.rb` | 1-152 | `Buffer`. content + ranges + `pos_to_loc` / `loc_to_pos` |
| `lib/rbs/source.rb` | 1-99 | `Source::RBS`, `Source::Ruby`. buffer + directives + decls |
| `lib/rbs/parser_aux.rb` | 1-142 | Entry points such as `Parser.parse_signature` |

### Referenced in M3 (**most important**)

| Path | Lines | Contents |
|---|---|---|
| `lib/rbs/environment.rb` | 277-372 | `insert_rbs_decl`. AST → entries registration. Port the equivalent to Rust |
| `lib/rbs/environment.rb` | 374-453 | `insert_ruby_decl`. Registration for Ruby-derived sources |
| `lib/rbs/environment.rb` | 455-468 | `add_source`. Dispatches `insert_rbs_decl` / `insert_ruby_decl` |
| `lib/rbs/environment.rb` | 500-560 | `resolve_signature`, `resolve_type_names`. **Replace with side-table approach in Rust** |
| `lib/rbs/environment.rb` | 568-575 | `append_context`. Context composition |
| `lib/rbs/environment.rb` | 577-711 | `resolve_declaration`, `resolve_member`, `resolve_method_type`, `resolve_type_params` |
| `lib/rbs/environment.rb` | 982-991 | `absolute_type_name`, `absolute_type`. Type-tree resolution |
| `lib/rbs/resolver/type_name_resolver.rb` | 1-169 | `TypeNameResolver`. `resolve`, `resolve_namespace0`, cache |
| `lib/rbs/environment/use_map.rb` | 1-77 | `UseMap`, `UseMap::Table`. `use` clause handling |
| `lib/rbs/environment/class_entry.rb` | 1-69 | `ClassEntry` |
| `lib/rbs/environment/module_entry.rb` | 1-66 | `ModuleEntry` |

### Referenced in M4 / M5

| Path | Lines | Contents |
|---|---|---|
| `lib/rbs/environment.rb` | 1002-1025 | `unload`. Foundation for M5 incremental updates |
| `lib/rbs.rb` | All | Gem load order |

## AST and type class hierarchy

| Path | Contents |
|---|---|
| `lib/rbs/ast/declarations.rb` | `Class`, `Module`, `Interface`, `TypeAlias`, `Constant`, `ClassAlias`, `ModuleAlias`, `Global` |
| `lib/rbs/ast/members.rb` | `MethodDefinition`, `Include`, `Extend`, `Prepend`, `AttrReader`, etc. |
| `lib/rbs/ast/directives.rb` | `Use`, `ResolveTypeNames` |
| `lib/rbs/ast/type_param.rb` | `TypeParam` |
| `lib/rbs/ast/annotation.rb` | `Annotation` |
| `lib/rbs/ast/comment.rb` | `Comment` |
| `lib/rbs/types.rb` | `Types::*` (17 variants). Each has `map_type_name` |
| `lib/rbs/type_name.rb` | `TypeName` |
| `lib/rbs/namespace.rb` | `Namespace` |

## C / Rust data definitions

| Path | Contents |
|---|---|
| `config.yml` | Source of truth for C / Rust AST node definitions. Consumed by `ruby-rbs/build.rs` and `templates/` |
| `include/rbs/ast.h` | Generated C AST structs |
| `include/rbs/util/rbs_constant_pool.h` | Constant pool (interning) |

## Reference CI workflows

| Path | Contents |
|---|---|
| `.github/workflows/rust.yml` | Existing Rust CI. Reference for librbs CI |
| `.github/workflows/ruby.yml` | Ruby gem CI |
| `.github/workflows/c-check.yml` | C code formatting check |

## Existing Rake tasks

`vendor/rbs/Rakefile`:

| Task | Contents |
|---|---|
| `rust:rbs:sync` | Regenerates `rust/{ruby-rbs-sys,ruby-rbs}/vendor/rbs/` from the pinned version |
| `rust:rbs:pin[VERSION]` | Updates the pinned version and re-syncs |
| `rust:rbs:symlink` | For development: symlinks `rust/.../vendor/rbs/` to repo root |

librbs places a similar set of tasks in **its own root Rakefile** when needed.

## Key numbers

- Files in core + stdlib: 247
- Total lines in core + stdlib: 134,228
- All RBS files parse successfully under `rust/ruby-rbs/tests/sanity.rs`

## Primary RBS call graph (simplified)

```
EnvironmentLoader#load
  ├─ each_signature
  │    ├─ each_dir
  │    │    └─ Repository#lookup
  │    └─ FileFinder.each_file
  │         └─ Parser.parse_signature   (C extension)
  └─ env.add_source(Source::RBS.new(...))
       └─ insert_rbs_decl (recursive)

Environment#resolve_type_names
  ├─ Resolver::TypeNameResolver.build(self)
  ├─ UseMap::Table.new + compute_children
  ├─ for each rbs_source:
  │    └─ resolve_signature
  │         └─ resolve_declaration (recursive)
  │              ├─ absolute_type_name → resolver.resolve
  │              ├─ absolute_type → type.map_type_name → absolute_type_name
  │              └─ resolve_member → resolve_method_type → ...
  └─ env.add_source (re-runs over reconstructed decls) ← ★ second build
```

The `★` second `add_source` is what we **eliminate** with the Rust-side
side-table. This is the project's primary winning move.
