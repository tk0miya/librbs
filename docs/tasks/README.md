# librbs Implementation Plan

This directory contains the planning documents for **librbs**, a project that
accelerates the RBS loader using Rust. Implementation will proceed in separate
sessions, milestone by milestone. Each agent picking up a milestone should read
this README and the linked documents before starting work.

## One-line summary

A gem that, simply by `require "librbs"`, makes
`RBS::Environment.from_loader` and `#resolve_type_names` significantly faster
through a Rust-backed implementation. User code is not modified.

## Document map

| File | Contents |
|---|---|
| [design.md](design.md) | Overall design, architectural decisions, and rationale |
| [reference.md](reference.md) | Locations in `vendor/rbs` to reference |
| [milestones/M1-skeleton.md](milestones/M1-skeleton.md) | Skeleton setup |
| [milestones/M2-discovery-and-parse.md](milestones/M2-discovery-and-parse.md) | Parallel file discovery and parsing |
| [milestones/M3-environment-and-resolver.md](milestones/M3-environment-and-resolver.md) | Side-table resolver and compatibility tests (**main milestone**, split into [M3a–M3f](milestones/M3/README.md)) |
| [milestones/M4-decision-point.md](milestones/M4-decision-point.md) | Benchmark measurement and next-phase decision |
| [milestones/M5-incremental.md](milestones/M5-incremental.md) | In-process incremental updates |
| [followups.md](followups.md) | Items deferred from a milestone, with the trigger that should pull them in |

## Background

- The repository currently contains nothing except `vendor/rbs/` (the upstream
  RBS gem source, pinned at v4.0.2).
- The upstream Rust crates (`ruby-rbs`, `ruby-rbs-sys`) already provide a C
  parser FFI and a Rust AST. We **depend on them as published crates from
  crates.io**; we never copy or fork them. The mirrored sources at
  `vendor/rbs/rust/{ruby-rbs,ruby-rbs-sys}/` exist purely for code reading
  and reference — they are not part of the build graph. Upstream version
  bumps are made by editing the version constraints in our `Cargo.toml`.

## Non-goals

The following are **deliberately out of scope**. Implementation must not add
them.

- **Persistent (disk) cache**: Because of Ruby's open-class semantics and
  inheritance lookup, a single-file change can invalidate resolution for the
  entire environment. Persistent caching cannot be operated soundly. Only
  in-process, in-memory reuse is permitted.
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
5. **Use a single lazy boundary first** (full materialization on the first
   `*_decls` access). Move to a two-tier boundary only after the M4 decision
   point.

## Pre-flight checklist for agents

Before starting a new milestone:

1. Read this README.
2. Read [design.md](design.md).
3. Read the document for the milestone you are picking up.
4. Consult [reference.md](reference.md) for relevant code in `vendor/rbs`.
5. Verify with git that the previous milestone's "Acceptance" criteria are met.
