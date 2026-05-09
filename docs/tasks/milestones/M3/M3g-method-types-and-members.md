# M3g: Method types + members (`RBS::MethodType`, `RBS::AST::Members::*`)

## Goal

Materialize every `RBS::AST::Members::*` variant and the `RBS::MethodType`
that method overloads carry. After this slice, given any class/module
body member node from the Rust AST, we can produce the matching Ruby
member object — but the surrounding `RBS::AST::Declarations::*` and
`Environment::*Entry` building still waits for M3h.

## Prerequisites

- M3f merged. `materialize/type_.rs` and `materialize/type_param.rs`
  exist with full unit coverage.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  section "AST → Ruby conversion".

## Scope

### `ext/librbs/src/materialize/method_type.rs`

`RBS::MethodType.new(type_params:, type:, block:, location:)`:

- `type_params` → reuse M3f's `materialize_type_param`.
- `type` → must be a `Function` or `UntypedFunction` — call into M3f.
- `block` → optional `RBS::Types::Block.new(type:, required:, self_type:)`,
  also reachable from `ProcType` so the helper lives here and is reused.

### `ext/librbs/src/materialize/member.rs`

Mirror the exhaustive list in `crates/librbs-core/src/resolver/driver.rs::walk_member`.
No wildcard fallthroughs — every variant is enumerated.

| AST node | Ruby class | Notes |
|---|---|---|
| `MethodDefinition` | `RBS::AST::Members::MethodDefinition` | overloads → array of `MethodDefinition::Overload.new(method_type:, annotations:)`; kind `:instance`/`:singleton`/`:singleton_instance`; visibility `:public`/`:private`/nil; `overloading?` flag; comment + annotations |
| `AttrAccessor` | `RBS::AST::Members::AttrAccessor` | name, type, ivar_name (Unspecified→nil / Empty→`false` / Name→Symbol); kind/visibility; comment + annotations |
| `AttrReader` | `RBS::AST::Members::AttrReader` | same as accessor |
| `AttrWriter` | `RBS::AST::Members::AttrWriter` | same as accessor |
| `InstanceVariable` | `RBS::AST::Members::InstanceVariable` | name, type, location, comment |
| `ClassInstanceVariable` | `RBS::AST::Members::ClassInstanceVariable` | name, type, location, comment |
| `ClassVariable` | `RBS::AST::Members::ClassVariable` | name, type, location, comment |
| `Include` | `RBS::AST::Members::Include` | mixin name (absolutize via Resolution) + args + annotations + comment |
| `Extend` | `RBS::AST::Members::Extend` | same shape as Include |
| `Prepend` | `RBS::AST::Members::Prepend` | same shape as Include |
| `Alias` | `RBS::AST::Members::Alias` | new_name, old_name, kind `:instance`/`:singleton`; comment + annotations |
| `Public` | `RBS::AST::Members::Public` | location only |
| `Private` | `RBS::AST::Members::Private` | location only |

Annotations: `RBS::AST::Annotation.new(string:, location:)`. Comments:
`RBS::AST::Comment.new(string:, location:)`. Both are reused from
declarations in M3h, so the helpers live in `member.rs` and are
re-exported.

### Test entry points (temporary)

```rust
fn _materialize_first_method_type(env: Value) -> Result<Value, Error>;
fn _materialize_first_member(env: Value) -> Result<Value, Error>;
```

Removed at M3h.

### Tests

`spec/unit/materialize_method_type_spec.rb`:

- Plain method type, with/without type_params, with/without block,
  block required vs optional, with self_type binding.
- Compare against `RBS::Parser.parse_method_type(text).to_json`.

`spec/unit/materialize_member_spec.rb`:

- One example per row of the member table.
- For attrs: cover the three `ivar_name` shapes (Unspecified / Empty /
  Name).
- For methods: instance / singleton / `self?.` (singleton_instance),
  public / private / unspecified visibility, `overloading?` true and
  false, with annotations + leading comment.
- For mixins: cover both unresolved and resolved envs to confirm
  Resolution lookup is wired through `materialize_type_name`.

## Out of scope (deferred)

- `RBS::AST::Declarations::*` and `Environment::*Entry` → M3h.
- Accessor patches and `materialize_all` cut-over → M3h.

## Acceptance

- [x] Every member variant in the table has a unit test that passes
      against pure-RBS JSON output.
- [x] `materialize/member.rs` matches `walk_member` exhaustively
      (no wildcard fallthroughs).
- [x] All M3a–M3f specs remain green; canonical-dump compat still
      `pending` (unblocked at M3h).

## References

- `vendor/rbs/lib/rbs/ast/members.rb`
- `vendor/rbs/lib/rbs/method_type.rb`
- `vendor/rbs/lib/rbs/ast/annotation.rb`
- `vendor/rbs/lib/rbs/ast/comment.rb`
- `crates/librbs-core/src/resolver/driver.rs::walk_member`
