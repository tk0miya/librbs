# M5: In-Process Incremental Updates

## Goal

Enable **minimal re-resolution on file edits** for long-lived processes such
as LSP servers. When the impact range of an edit cannot be determined
soundly, fall back to full re-resolution to preserve correctness.

No disk persistence (see non-goals in [../README.md](../README.md)).

## Prerequisites

- M4 is complete (M4a included if it was required).
- Benchmarks confirm the speedup achieved by M3 / M4.

## Motivation

Steep's and RBS's LSP implementations repeatedly call `Environment#unload`
and `add_source` against edited files. Through M4, this path:

- Reconstructs the env (handle and all) on every `unload`.
- Re-runs `add_source` to rebuild entries.
- Re-runs `resolve_type_names` to recompute resolution.

A full rebuild on every edit is expensive. M5 introduces an optimization
that **keeps resolution results for sources that weren't touched**.

### Reference: Steep's actual usage pattern

Steep's `SignatureService#update_env`
(`lib/steep/services/signature_service.rb`, ~L302) batches a single
edit cycle as the following triple, always called together:

```ruby
env = latest_env.unload(paths)               # remove decls of edited files
updated_files.each_value { |c| env.add_source(c.source_or_self) }
env.validate_type_params
env = env.resolve_type_names(only: new_decls)  # ← only: is mandatory
```

Findings that constrain this milestone:

1. **`resolve_type_names(only:)` is already wired up by M3d** (`fn
   resolve_type_names(env, only)` + the Ruby patch with `only: nil`).
   Steep always passes the set of newly added declarations, so M5 must
   keep that path working — the incremental fast-path described in
   Task 3 must compose with `only:`, not replace it.
2. **`add_source` receives an already-parsed `RBS::Source::RBS` or
   `RBS::Source::Ruby`** (not raw text). Steep does its own parsing.
3. **Edits are batched** via `ChangeBuffer` before reaching `update_env`,
   so `unload` taking an array of paths is the right shape.
4. **Steep also does its own invalidation** at the `DefinitionBuilder`
   level using `RBS::AncestorGraph` (`update_builder`, ~L377), independent
   of what `Environment` returns. Our env-internal partial resolution is
   purely additive — it does not need to expose any new diff information
   to consumers.
5. **A new `Environment` instance is expected per edit.** Steep replaces
   `latest_builder.env` wholesale, so identity changes are not a concern.

## Design principles

### Safety first

Ruby's open classes and inheritance mean **any file might affect any class**.
Optimistic partial re-resolution risks soundness violations. M5 policy:

1. **Default to safe**: when in doubt, full re-resolution.
2. **Limited optimization**: keep other sources' resolution results only
   when **no new type names were introduced and the alias table didn't
   change**.
3. **Fallback on inconsistency**: if any inconsistency is detected on the
   optimistic path, fall back to full re-resolution.

### Detecting partial-resolution eligibility

Before/after a source edit, check whether:

- The set of **TypeNames defined by the source** changed.
- The **alias table contributed by the source** changed.

If both are unchanged, other sources' resolution results are **theoretically
invariant** (resolution depends only on `all_names` and `aliases`). If
either changed, full re-resolve.

### API

Behavior must remain compatible with the existing `RBS::Environment`'s
`unload` and `add_source`. Do not introduce new public API.
`resolve_type_names(only:)` is already in place from M3d; M5 only
changes its internals to take an incremental path when possible.

## Tasks

### 1. Patch `unload`

```ruby
# add to lib/librbs/patches/environment.rb
def unload(paths)
  Librbs::Native.unload(self, paths.map(&:to_s))
end
```

Native implementation:

1. Identify which entries in `env.sources` correspond to the supplied paths.
2. Build a new env without them (rebuild arena/entries; discard resolution).
3. Return a new `RBS::Environment` carrying the new handle.

Initially, **just discard resolution** (so the next `resolve_type_names`
does a full re-resolve). Optimization comes next.

### 2. Patch `add_source`

```ruby
def add_source(source)
  Librbs::Native.add_source(self, source)
end
```

`source` is an already-parsed `RBS::Source::RBS` or `RBS::Source::Ruby`
(Steep parses files itself before calling this). The native side must
extract `source.declarations` and ingest them into a freshly built env;
do not attempt to re-parse from text.

The native implementation is otherwise similar to `unload`: rebuild env,
discard resolution.

### 3. Partial re-resolution optimization

The Ruby patch and the `fn resolve_type_names(env, only)` Magnus entry
point already exist (M3d). M5 only changes the **native body** to take
an incremental path when a previous resolution is reachable through
the env handle.

Native logic:

```rust
pub fn resolve_incremental(
    prev: &Environment,
    next: &Environment,
    only: Option<&DeclSet>,   // Steep passes the newly added decls
) -> Result<Resolution, Error> {
    // If prev has no resolution, fall back to a normal resolve.
    let Some(prev_resolution) = &prev.resolution else {
        return resolve(next, only);
    };

    // Diff all_names / aliases.
    let prev_names: FxHashSet<_> = prev.all_type_names();
    let next_names: FxHashSet<_> = next.all_type_names();

    if prev_names != next_names {
        return resolve(next, only);   // full re-resolve
    }

    if prev.aliases() != next.aliases() {
        return resolve(next, only);   // full re-resolve
    }

    // Only the source bodies changed; clone prev's resolution and
    // re-resolve only the modified sources.
    let mut resolution = prev_resolution.as_ref().clone();
    let changed_sources = prev.diff_sources(next);

    for source_idx in changed_sources {
        resolution.clear_for_source(source_idx);
        resolve_source_into(&next.sources[source_idx], &mut resolution, ...);
    }

    Ok(resolution)
}
```

Notes:

- `only:` is plumbed in by M3d. On the **incremental path** the
  filter is implicitly satisfied because we re-resolve exactly the
  changed sources (a superset of `only`). On the **full fallback** we
  forward `only:` to the existing M3d driver unchanged.
- We track `prev` via the env handle threaded through Ruby (each
  `unload`/`add_source` returns a new env that retains a reference to
  its predecessor's resolution). No global cache.

### 4. Tests

```ruby
# spec/incremental_spec.rb
RSpec.describe "incremental resolution" do
  it "preserves resolution when only source content changes" do
    env1 = RBS::Environment.from_loader(loader).resolve_type_names
    dump1 = canonical_dump(env1)

    # Re-load the same content under a different path, then unload.
    env2 = env1.unload([target_path]).add_source(reloaded_source)
    env2 = env2.resolve_type_names

    expect(canonical_dump(env2)).to eq(dump1)
  end

  it "falls back to full resolve when type names change" do
    # ...
  end
end
```

### 5. Benchmark

LSP simulation — mirror Steep's `update_env` triple so the numbers
correspond to one real edit cycle:

```ruby
# benchmark/lsp_simulation.rb
env = RBS::Environment.from_loader(loader).resolve_type_names

# Repeat "edit one file" 100 times
100.times do |i|
  new_decls = edited_source.declarations
  env = env.unload([edited_path])
  env = env.add_source(edited_source)
  env = env.resolve_type_names(only: new_decls)
end
```

Compare pure RBS vs librbs (incremental ON / OFF).

## Acceptance

- [ ] `unload` / `add_source` patches do not break existing tests.
- [ ] Correctness tests for partial re-resolution are green.
- [ ] LSP simulation is meaningfully faster than pure RBS.
- [ ] Fallbacks are intact (edits that change type names trigger a full
      re-resolve).
- [ ] Smoke test against Steep: a real Steep run using librbs produces
      the same diagnostics as pure RBS for at least one sample project
      (e.g. `steep/smoke`).

## Out of scope for this milestone

- Persistent caching (a non-goal).
- File watching.
- LSP protocol implementation (that's Steep's job).

## Pitfalls and mitigation

### Handle mutability

Until M4, the Environment handle was an immutable `Arc<Environment>`.
`unload` / `add_source` produce a new handle. Make sure the identity change
of the Ruby `RBS::Environment` instance is acceptable to consumers like
Steep.

The Ruby version also returns a new env from `unload` / `add_source`, so
this should be a non-issue.

### Concurrent access

Confirm whether concurrent access is expected in LSP usage. Ruby's GVL
serializes most things, but ensure no simultaneous access to fields inside
`magnus::TypedData`. Decide whether to guard with `RwLock` or to require
single-threaded access on the Ruby side.
