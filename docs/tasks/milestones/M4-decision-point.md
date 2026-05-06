# M4: Benchmark Measurement and Next-Phase Decision

## Goal

Measure performance at the M3 completion point and **decide whether to
proceed with per-Entry lazy materialization (M4a)**.

This milestone is primarily about **measurement, decision, and recording**
rather than implementation. The decision branches into M4a or M4b.

## Prerequisites

- M3 is complete.
- `from_loader → resolve_type_names → class_decls.size` works end-to-end on
  both pure RBS and librbs.

## Tasks

### 1. Set up the benchmark suite

Create the following under `benchmark/`:

```
benchmark/
├── load_only.rb          # from_loader only
├── load_and_resolve.rb   # from_loader + resolve_type_names
├── full_use.rb           # adds class_decls.size to trigger materialization
├── steep_simulation.rb   # mimics real Steep usage (see below)
└── helpers.rb
```

Each script uses `benchmark-ips` to compare two cases:

- Pure RBS (do not `require "librbs"`).
- librbs (do `require`).

### 2. Measurement matrix

Three input sizes:

| Case | Contents |
|---|---|
| **small** | core only |
| **medium** | core + stdlib |
| **large** | core + stdlib + major gems (json, set, bigdecimal, csv, activesupport, etc.) |

That's 3 sizes × 4 benchmark scripts = 12 numbers.

### 3. Steep usage simulation

To gauge real-world load:

```ruby
# benchmark/steep_simulation.rb
loader = RBS::EnvironmentLoader.new
loader.add(library: "rbs")
env = RBS::Environment.from_loader(loader).resolve_type_names

# Reproduce Steep's pattern of pulling class_decls for every type name
env.class_decls.each_value do |entry|
  entry.each_decl do |decl|
    decl.type_params if decl.respond_to?(:type_params)
    decl.members.each { |m| m } if decl.respond_to?(:members)
  end
end
```

Reading Steep's actual source to confirm how `Environment` is used is
preferable. Use the parallel investigation findings.

### 4. Record results

Write results to `benchmark/results/M4-baseline.md`:

```markdown
# M4 baseline benchmark

Date: YYYY-MM-DD
Environment: macOS 14.x / Ruby 3.4.x / Apple M2 (or Linux x86_64 / ...)

## load_and_resolve.rb

| size | pure RBS | librbs (M3) | speedup |
|---|---|---|---|
| small | XXX ms | XXX ms | X.Xx |
| medium | XXX ms | XXX ms | X.Xx |
| large | XXX ms | XXX ms | X.Xx |

## full_use.rb

...

## steep_simulation.rb

...
```

### 5. Decision

Use this flow to choose the next step:

```
Look at the speedup factors of load_and_resolve and full_use:

- load_and_resolve >= 2x AND full_use >= 2x:
    → M4a (per-Entry lazy materialization) is unnecessary.
       Proceed to M4b (compatibility-API completion). Goal achieved.

- load_and_resolve >= 3x AND full_use < 1.5x:
    → Materialization is the bottleneck. M4a is worth doing.

- load_and_resolve < 1.5x:
    → Something is wrong with M3 (the Rust port of the resolver isn't
       paying off). Re-investigate M3 with a profiler.
```

Record the decision in `benchmark/results/M4-decision.md`:

- The numbers
- Which case is how many times faster
- Whether to proceed with M4a or M4b
- The reasoning

### 6. M4a path: additional implementation

Per-Entry lazy materialization:

- Patch `RBS::Environment::ClassEntry` etc. so that `each_decl` /
  `context_decls` / `primary_decl` materialize their decls on first call.
- Replace `materialize_all` with a coarser `materialize_class_decls_keys`
  Native API that creates only the keys and Entry shells.
- Give Entries a Rust handle ivar and lazy-materialize per method.

Detailed design happens at decision time. This document only sketches the
shape.

### 7. M4b path: compatibility-API completion

Cover other `RBS::Environment` methods that Steep / the investigation
revealed:

- `each_type_name`
- `validate_type_params`
- `each_rbs_source` / `each_ruby_source`
- `inspect`
- `buffers`
- ...

These should work via the librbs path. Most should be handled by routing
through `ensure_materialized`, requiring minimal additional code.

## Acceptance

- [ ] `benchmark/results/M4-baseline.md` records all 12 numbers.
- [ ] `benchmark/results/M4-decision.md` records the decision and reasoning.
- [ ] Either M4a or M4b is implemented.
- [ ] Manually verify that running Steep on a real project produces the
      same results as before the change.

## Pitfalls and mitigation

### benchmark-ips assumptions

GC and JIT skew the first run. Trust `Benchmark.ips`'s warmup and run
enough iterations. Or, use `Benchmark.realtime` to measure **cold-start
time**, which is more representative for loader code (cold start matters
more than steady-state for libraries that load once).

Report both numbers.

### Subprocess isolation

Comparing pure RBS and librbs in the same process is impossible (require
order interferes). Run each case in a separate Ruby process and have the
script print `Benchmark.realtime`:

```ruby
# benchmark/helpers.rb
def measure_subprocess(libs, script)
  full = libs.map { |l| "require '#{l}'" }.join("\n") + "\n" + script
  out, _, _ = Open3.capture3("ruby", "-e", full)
  out.to_f  # the script puts realtime at the end
end
```

### Selecting major gems for the large case

Pick gems **commonly used in projects that adopt Steep**. Adding obscure
gems doesn't strengthen the decision basis. Rule of thumb: target Rails
projects with `activesupport`, `actionpack`, `actionmailer`, etc.

## Next milestone

Depending on the outcome:
- If M4a is required, design and implement it, then move on to M5.
- Otherwise, go directly to M5.

→ [M5-incremental.md](M5-incremental.md)
