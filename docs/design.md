# librbs Design Document

## 1. Problem statement

The chain of operations starting from `RBS::EnvironmentLoader#load` is slow.
In particular, **`RBS::Environment#resolve_type_names` dominates the cost**.
Real-world consumers always work with a resolved environment, so optimizing
loading alone is insufficient.

### 1.1 Hot path of loading

| Step | Ruby implementation | Cost driver |
|---|---|---|
| (1) File discovery | `EnvironmentLoader#each_dir`, `FileFinder.each_file` | `Pathname.glob("**/*.rbs")` |
| (2) File read | `Buffer.new(content: path.read(...))` | UTF-8 decoding, Ruby String allocation |
| (3) Parse | `Parser.parse_signature` (C extension) | The C parser itself is fast |
| (4) **C AST → Ruby AST translation** | `ext/rbs_extension/ast_translation.c` | **Largest spike of Ruby object allocation** |
| (5) Environment registration | `Environment#add_source` → `insert_rbs_decl` | Recursive AST descent, Hash inserts |
| (6) **Type-name resolution** | `Environment#resolve_type_names` | **Full AST reconstruction + a second `add_source`** |

### 1.2 Why `resolve_type_names` is expensive

The implementation at `vendor/rbs/lib/rbs/environment.rb:522-560`:

1. **Allocates a fresh `Environment`** and walks every source.
2. Through `resolve_declaration` / `resolve_member` / `absolute_type` /
   `type.map_type_name`, **regenerates every AST node immutably** (only the
   type names change).
3. Calls `add_source` again on the new environment with the regenerated decls.

So pure Ruby builds:
- The AST twice (once after parse, once after resolve).
- The environment twice (once via `loader.load`, once inside
  `resolve_type_names`).

These layered costs dominate total time.

## 2. Design core

### 2.1 Eliminating AST reconstruction with a side-table

**Store resolution results in a separate table keyed by AST node ID.** The AST
itself stays immutable.

```
Rust side:
  sources:    Vec<Source>                          ← AST nodes owned by ruby-rbs's ManagedParser
  resolution: HashMap<NodeId, ResolvedTypeName>    ← the side-table

Ruby side: no logic changes
  env.resolve_type_names still appears to "return a new env"
```

This eliminates:
- The second AST construction (the AST-regeneration part of step 6).
- The second `add_source` (the env-rebuild part of step 6).

`resolve_type_names` becomes "compute the resolution side-table and swap the
handle".

### 2.2 Pure-Rust Environment

```rust
struct Environment {
    interner:   Arc<TypeNameInterner>,       // (parent_id, segment_id) → Sym
    sources:    Arc<Vec<Source>>,            // Buffer + decl node references (AST owned by ruby-rbs ManagedParser)
    entries:    Arc<Entries>,                // integer-keyed class_decls etc.
    resolution: Option<Arc<Resolution>>,     // None: unresolved / Some: resolved
}
```

`from_loader` returns a handle with `resolution: None`.
`resolve_type_names` returns a **new handle** with `resolution: Some(...)`
swapped in (sources and entries are shared via `Arc`, so the actual cost
is one handle).

### 2.3 Hash-consing TypeName

Compress `TypeName` to `Sym(u32)` internally:

- `(parent_ns_id, last_segment_id) → ns_id` hash-consing keeps `with_prefix`
  to integer arithmetic.
- `Hash[TypeName]` keys in Ruby compare by `Namespace#path` `Array` equality,
  which is expensive. Inside Rust we use `FxHashMap<Sym, ...>`, near
  copy-free.
- `Resolver::cache` keys become `(Sym, Sym)`.

### 2.4 Parallel resolution

After the resolver inputs (`all_names` set + `aliases` table) are fixed, each
source's resolution becomes an **independent write-only job over a
read-only resolver** (writing into a disjoint region of the side-table). We
distribute these via `rayon::par_iter`.

This parallelism is unavailable in Ruby because of the GVL.

### 2.5 Single lazy boundary

To keep implementation complexity manageable, M3 fixes **a single lazy
boundary**:

- `from_loader` → returns a Rust-backed `RBS::Environment` (zero Ruby
  materialization).
- `resolve_type_names` → pure Rust, returns a new Rust-backed
  `RBS::Environment` (zero Ruby materialization).
- **The first call to any of `class_decls`, `interface_decls`,
  `type_alias_decls`, `constant_decls`, `class_alias_decls`, or
  `global_decls` materializes all six Hashes at once** (every Entry,
  every Decl, and every Type tree converted into the genuine `RBS::*`
  classes).
- Subsequent calls return the memoized `Hash`.

The two-tier boundary (per-Entry lazy) is deferred to M4, gated on
benchmarks.

### 2.6 No persistent cache

Ruby's open-class semantics and inheritance lookup mean that a single-file
change may invalidate the entire environment's resolution. Persistent caching
cannot be operated soundly. We restrict ourselves to in-process, in-memory
reuse.

## 3. Interface strategy

### 3.1 What users see

```ruby
require "rbs"
require "librbs"   # this is all

loader = RBS::EnvironmentLoader.new
loader.add(library: "json")
env = RBS::Environment.from_loader(loader)
env = env.resolve_type_names

# From here it's the normal RBS API. Types, return values, and exceptions
# are fully compatible.
env.class_decls         # => Hash[RBS::TypeName, RBS::Environment::ClassEntry]
env.class_decls[name].each_decl.first  # => RBS::AST::Declarations::Class
```

Users never have to think about the `Librbs` module.

### 3.2 Patch targets

| Target | Purpose |
|---|---|
| `RBS::EnvironmentLoader#load` | Delegate discovery + parse to Rust |
| `RBS::Environment.from_loader` | Build the env in Rust. Return an `RBS::Environment` carrying a Rust handle |
| `RBS::Environment#resolve_type_names` | Compute resolution in Rust. Return a new Rust-backed `RBS::Environment` |
| `RBS::Environment#class_decls` and 5 siblings | Materialize all six Hashes on first access; memoize thereafter |
| `RBS::Environment#add_source`, `#unload` | Used in M5 incremental updates |

Implementation style: `Module#prepend`, leaving the original implementation
reachable via `super`. If the native extension fails to load, **do not apply
the patches; let pure RBS take over** (warn only, never crash).

### 3.3 Return types

All return values are genuine RBS classes. We do not return custom types:

- `class_decls.keys` → `Array[RBS::TypeName]`
- `class_decls[name]` → `RBS::Environment::ClassEntry`
- `entry.each_decl` → `RBS::AST::Declarations::Class`, etc.
- `decl.members` → `Array[RBS::AST::Members::*]`
- `member.types[0]` → `RBS::Types::*`

## 4. Repository layout (post-implementation)

```
librbs/
├── Cargo.toml                          # workspace
├── crates/
│   ├── ruby-rbs-sys/                   # copied from vendor/rbs/rust/ruby-rbs-sys
│   ├── ruby-rbs/                       # copied from vendor/rbs/rust/ruby-rbs
│   ├── librbs-core/                    # new: discovery / env / resolver
│   └── librbs-ruby/                    # new: magnus extension
├── lib/
│   ├── librbs.rb                       # require entry; applies patches
│   └── librbs/
│       ├── version.rb
│       └── patches/
│           ├── environment_loader.rb
│           ├── environment.rb
│           └── entry.rb
├── ext/
│   └── librbs/
│       └── extconf.rb                  # rb-sys + cargo
├── sig/                                # type signatures for librbs itself
├── spec/
│   ├── unit/
│   └── compat/                         # canonical-dump equivalence tests vs pure RBS
├── benchmark/
├── vendor/
│   └── rbs/                            # existing upstream RBS subtree
├── Steepfile
├── Gemfile
├── librbs.gemspec
├── Rakefile
└── .github/workflows/ci.yml
```

The duplication of `vendor/rbs/rust/` into `crates/ruby-rbs-sys` and
`crates/ruby-rbs` can start as a **physical copy** (we will consider
subtree/scripted approaches later).

## 5. Crate responsibilities

### 5.1 `librbs-core` (Ruby-independent)

```
crates/librbs-core/src/
├── lib.rs
├── interner.rs        # TypeName / Symbol interning
├── discovery/
│   ├── mod.rs
│   ├── repository.rs  # GemRBS / VersionPath equivalents
│   └── walker.rs      # rayon-driven file discovery
├── source.rs          # Buffer + line info, Source::RBS equivalent
├── env/
│   ├── mod.rs         # Environment struct
│   ├── entry.rs       # ClassEntry / ModuleEntry / ...
│   ├── insert.rs      # insert_rbs_decl equivalent
│   └── use_map.rs
├── resolver/
│   ├── type_name.rs
│   └── constant.rs
├── canonical.rs       # canonical dump for compatibility tests
└── error.rs
```

`cargo test` covers the entire crate without Ruby. All logic can be verified
in pure Rust.

### 5.2 `librbs-ruby` (magnus FFI)

`#[magnus::init]` defines `Librbs::Native::*`. `TypedData` wraps
`Arc<Environment>`. GC marking is implemented via `DataTypeFunctions`.

The native API is intentionally minimal:

```rust
// conceptual
fn build_environment(loader: Value) -> Result<Value, Error>;     // returns RBS::Environment-compatible object
fn resolve_type_names(env: Value, only: Option<Value>) -> Result<Value, Error>;
fn materialize_all(env: Value) -> Result<(), Error>;             // build all six Hashes at once
```

The native API stays inside `Librbs::Native` and is not exposed to end users.

### 5.3 Ruby patch layer

A set of `Module#prepend`-based patches, triggered by `require "librbs"`:

```ruby
# lib/librbs.rb
require "rbs"
require "librbs/version"

begin
  require "librbs/librbs"   # native extension produced by ext/
  require "librbs/patches/environment_loader"
  require "librbs/patches/environment"
  require "librbs/patches/entry"
rescue LoadError => e
  warn "[librbs] native extension failed to load: #{e.message}"
  warn "[librbs] falling back to pure RBS implementation"
end
```

## 6. Toolchain and distribution

| Item | Choice |
|---|---|
| Ruby ↔ Rust bridge | `magnus` + `rb-sys` |
| Extension build | `rb-sys-build` + `rake-compiler`; `extconf.rb` invokes `cargo build --release` |
| Distribution | `oxidize-rb/cross-gem-action` for Linux x86_64/aarch64 and macOS x86_64/arm64 (4 targets) |
| Ruby support | 3.3 / 3.4 / 4.0 |
| Parallelism | `rayon` |
| Hashing | `rustc-hash` (`FxHashMap`) |

## 7. CI configuration

`.github/workflows/ci.yml`:

| Job | Contents |
|---|---|
| `rust-test` | matrix: ubuntu / macos × stable Rust. `cargo test --workspace` |
| `rust-lint` | `cargo fmt --check`, `cargo clippy -- -D warnings` |
| `ruby-test` | matrix: ruby 3.3 / 3.4 / 4.0 × ubuntu / macos. rake-compiler build + rspec |
| `compat-test` | **The core**. Run `from_loader → resolve_type_names` over core+stdlib+major gems and verify canonical-dump equality against pure RBS |
| `bench` | benchmark-ips comparison vs pure RBS, posted to PR comments (M3 onward) |
| `cross-gem-dryrun` | On release tags only: `oxidize-rb/cross-gem-action` build sanity check |

## 8. Compatibility guarantees

### 8.1 Canonical dump

A deterministic string representation of an `Environment`. Implemented in M3.
**Important: the dumper must also be implemented on the Rust side**, so that
comparisons can be done without going through Ruby materialization (a Ruby
detour would defeat the point of M3).

### 8.2 Comparison

```
Pure RBS:
  RBS::Environment.from_loader(loader).resolve_type_names.canonical_dump

librbs:
  RBS::Environment.from_loader(loader).resolve_type_names.canonical_dump
  ↑ after `require "librbs"`, this runs through Rust
```

Compare the two with `expect(rust_dump).to eq(ruby_dump)`.

### 8.3 Input matrix

- Core only
- Core + stdlib
- Core + stdlib + major gems (json, set, bigdecimal, csv, ...)

## 9. Rationale for milestone splits

Estimated cost distribution of loading:

| Step | Share | Eliminated by Rust? |
|---|---|---|
| (1) Discovery | A few % | Shortened by parallelism |
| (2) Parse | 5-10% | Shortened by parallelism |
| (3) C → Ruby AST translation | 30-40% | Cannot disappear without lazy materialization |
| (4) insert_rbs_decl | 10% | Significantly shortened by Rust |
| (5) AST reconstruction in resolve | 20-30% | **Eliminated** by side-table |
| (6) Second add_source | 10% | **Eliminated** |

We expect a **2-3x speedup** by M3 from side-table + Rust port.

Per-Entry lazy materialization (M4a) might add 1.5-3x on top, depending on
workload, but at the cost of implementation complexity. The decision is
deferred to post-M3 benchmarking.

## 10. Parallel investigation task

While M1 is in progress, investigate how Steep and similar consumers actually
use `RBS::Environment`:

- All call sites that treat `class_decls` as a `Hash`
- Usage patterns of `each_type_name`, `each_decl`, `validate_type_params`
- How `Environment#unload` is invoked (directly affects M5 design)
- Whether `Marshal.dump(env)` is ever used

Findings tighten the compatibility boundary defined in M3 and M4.
