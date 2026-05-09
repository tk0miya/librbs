# M3f: Types + type params (`RBS::Types::*`, `RBS::AST::TypeParam`)

## Goal

Materialize every `RBS::Types::*` variant and `RBS::AST::TypeParam`
from the Rust AST, end-to-end, with per-variant unit coverage. After
this slice, given any `ruby-rbs` type node, we can produce the
matching `RBS::Types::*` instance — but no member or declaration
materialization yet.

## Prerequisites

- M3e merged. `MaterializeCtx`, `materialize/location.rs`,
  `materialize/type_name.rs` are in place; the NodeId walk order is
  proven to match the resolver driver.
- Read [../M3-environment-and-resolver.md](../M3-environment-and-resolver.md)
  section "AST → Ruby conversion".

## Scope

### `ext/librbs/src/materialize/type_param.rs`

Build `RBS::AST::TypeParam.new(name:, variance:, upper_bound_type:,
default_type:, location:, lower_bound_type:, unchecked:)` from a
`TypeParamNode`:

- Variance: `TypeParamVariance::{Invariant, Covariant, Contravariant}`
  → upstream's `:invariant` / `:covariant` / `:contravariant` symbols.
- `upper_bound`, `lower_bound`, `default_type` are optional and
  recursively materialize via `materialize/type_.rs`.
- `unchecked` → boolean.
- Location and `name_location` sub-locations.

Used by both M3g (method types' `type_params`) and M3h (decl-level
`type_params`).

### `ext/librbs/src/materialize/type_.rs`

One arm per upstream `RBS::Types::*` constructor. Mirror the
exhaustive list in `crates/librbs-core/src/resolver/driver.rs::walk_type`
so any drift is caught at compile time (`_ =>` fallthroughs are
forbidden — list every variant explicitly).

| AST node | Ruby class | Notes |
|---|---|---|
| `BoolType` | `RBS::Types::Bases::Bool` | location only |
| `VoidType` | `RBS::Types::Bases::Void` | location only |
| `AnyType` | `RBS::Types::Bases::Any` | `todo:` flag |
| `NilType` | `RBS::Types::Bases::Nil` | location only |
| `TopType` | `RBS::Types::Bases::Top` | location only |
| `BottomType` | `RBS::Types::Bases::Bottom` | location only |
| `SelfType` | `RBS::Types::Bases::Self` | location only |
| `InstanceType` | `RBS::Types::Bases::Instance` | location only |
| `ClassType` | `RBS::Types::Bases::Class` | location only |
| `VariableType` | `RBS::Types::Variable` | name symbol + location |
| `LiteralType` | `RBS::Types::Literal` | child literal (Integer / String / Symbol / Bool) |
| `ClassInstanceType` | `RBS::Types::ClassInstance` | name + args; absolutize via Resolution |
| `InterfaceType` | `RBS::Types::Interface` | name + args; absolutize |
| `AliasType` | `RBS::Types::Alias` | name + args; absolutize |
| `ClassSingletonType` | `RBS::Types::ClassSingleton` | name only; absolutize |
| `TupleType` | `RBS::Types::Tuple` | child types |
| `UnionType` | `RBS::Types::Union` | child types |
| `IntersectionType` | `RBS::Types::Intersection` | child types |
| `RecordType` | `RBS::Types::Record` | hash of `Symbol → RecordFieldType` |
| `OptionalType` | `RBS::Types::Optional` | wraps one type |
| `ProcType` | `RBS::Types::Proc` | function + optional block + optional self_type |
| `BlockType` | (used by Proc / MethodType) | required flag + type + optional self_type |
| `FunctionType` | `RBS::Types::Function` | required/optional/rest/trailing positionals; required/optional/rest keywords; return |
| `UntypedFunctionType` | `RBS::Types::UntypedFunction` | return type only |

Helpers:

- `materialize_type(ctx, node) -> Value` is the dispatch entry point.
- Function/Proc parameters use `RBS::Types::Function::Param.new(type:, name:, location:)`.

### Test entry points (temporary)

```rust
fn _materialize_first_type_alias_target(env: Value) -> Result<Value, Error>;
fn _materialize_first_method_type_params(env: Value) -> Result<Value, Error>;
```

Each parses one source from the env, walks to the relevant node,
returns the materialized Ruby object. Removed at M3h.

### Tests

`spec/unit/materialize_type_spec.rb`:

- One example per row of the table above, using a single-line RBS
  fixture (`type t = ...`) that exercises that variant.
- Compare the materialized `Value.to_json` with
  `RBS::Parser.parse_type(text).to_json` for byte equality.
- Resolution variants: an unresolved env, a resolved env, and an
  env with `# resolve-type-names: false` directive — confirm the
  three Resolution states (None / Resolved / Unresolved) all reach
  the right `TypeName.absolute?` value.

`spec/unit/materialize_type_param_spec.rb`:

- Variance forms (`out T`, `in U`, `T`).
- With/without bounds and default.
- `unchecked` modifier.

## Out of scope (deferred)

- `RBS::MethodType` (the wrapping shape) and members → M3g.
- Decls + entries + cut-over → M3h.

## Acceptance

- [x] Every type variant in the table has a unit test that passes
      against `RBS::Parser.parse_type` JSON output.
- [x] `materialize/type_.rs` matches `walk_type` exhaustively (no
      wildcard fallthroughs).
- [x] Resolution lookup behaves correctly for the three states.
- [x] M3a–M3e specs remain green; canonical-dump compat still
      `pending` (unblocked at M3h).

## References

- `vendor/rbs/lib/rbs/types.rb`
- `vendor/rbs/lib/rbs/ast/type_param.rb`
- `crates/librbs-core/src/resolver/driver.rs::walk_type`
