# librbs Outstanding Work

This directory tracks the remaining planning work for **librbs**, a project
that accelerates the RBS loader using Rust. Completed milestones have been
removed; the design and reference documents now live one level up under
[../](../).

## One-line summary

A gem that, simply by `require "librbs"`, makes
`RBS::Environment.from_loader` and `#resolve_type_names` significantly faster
through a Rust-backed implementation. User code is not modified.

## Document map

| File | Contents |
|---|---|
| [../design.md](../design.md) | Overall design, architectural decisions, and rationale |
| [../reference.md](../reference.md) | Locations in `vendor/rbs` to reference |
| [followups.md](followups.md) | Items deferred from a prior milestone, with the trigger that should pull them in |

## Non-goals

The following are **deliberately out of scope**. Implementation must not add
them.

- **Persistent (disk) cache**: Because of Ruby's open-class semantics and
  inheritance lookup, a single-file change can invalidate resolution for the
  entire environment. Persistent caching cannot be operated soundly. Only
  in-process, in-memory reuse is permitted.
- **In-process incremental updates** (`unload` / `add_source` after a
  resolved env): out of scope.
- **Windows support**: Linux / macOS only for now. Do not introduce
  Windows-specific branches.
- **Public `Librbs::*` API**: Users must not interact with the `Librbs`
  namespace directly. `Librbs` is reserved for internal implementation.
- **Deviations from the `RBS::*` interface**: Return values, arguments, and
  exceptions must remain fully compatible with existing RBS. Do not return
  custom lazy classes.
- **Bridges other than `magnus`**: Fixed at `rb-sys` + `magnus`.
- **Custom Ruby AST**: Reuse `RBS::AST::*` and `RBS::Types::*`. Do not
  reimplement them.

## Core principles

1. **The interface is confined to monkey-patches on `RBS::*`**.
2. **`from_loader` and `resolve_type_names` are pure-Rust operations**. They
   produce zero Ruby AST/TypeName/Buffer instances.
3. **Measure speedups end-to-end**, covering loading through name resolution.
   Numbers from `from_loader` alone are not the basis for decisions.
4. **Compatibility is verified mechanically via canonical-dump diff**. Any
   deviation should fail tests.

