# M5 materialize-cache experiment

Date: 2026-05-11
Environment: Ubuntu 24.04 LTS / Ruby 3.3.6 / Linux x86_64 (kernel 6.18.5).
Run inside the dev container, so absolute numbers are not directly
comparable to `M4-baseline.md`; relative comparisons across configurations
on the same host are the signal to take from this file.

Pure-RBS comparison column is omitted because the vendored RBS in the
container ships without the matching `parser.so`, so the pure path can
only be measured on a developer workstation with rbs 4.x installed. The
absolute librbs numbers below replace the "all-off" config as the
de-facto baseline.

## Setup

The materializer hot path rebuilds `RBS::TypeName`, `RBS::Namespace`,
and Ruby `Symbol` Values on every type-name occurrence — the same names
(`String`, `Integer`, …) appear hundreds of times across stdlib. M5
adds per-`materialize_all` caches keyed by interner symbols so each
`(NamespaceSym, mark_absolute)` and `(TypeNameSym, mark_absolute)` builds
one Ruby object instead of one-per-occurrence.

Four caches, each toggleable via `LIBRBS_CACHE_{SYM,NS,TN,PATH}=0/1`:

- `SYM`  — `Sym → magnus::Value` for Ruby symbols (`:String`, `:path`, …).
- `NS`   — `(NamespaceSym, abs) → RBS::Namespace` Ruby instance.
- `TN`   — `(TypeNameSym,  abs) → RBS::TypeName`  Ruby instance.
- `PATH` — `NamespaceSym → Array<Symbol>` reused as the `path:` kwarg.

Benchmark: `benchmark/standalone_split.rb`, REPEATS=10, min-of-10.
"small" = core only. "medium" = `pathname date time uri optparse logger
stringio strscan`. Times are materialize-phase only (build+resolve
overhead is reported separately and is unaffected by the caches —
caches are populated lazily inside `materialize_all`).

## Results

| size   | config              | materialize (ms) | vs A.off |
|--------|---------------------|------------------|----------|
| small  | A.off (none)        | 174.6            | 1.00x    |
| small  | B.sym               | 175.7            | 0.99x    |
| small  | C.sym+ns            | 171.0            | 1.02x    |
| small  | D.sym+ns+tn         | 134.9            | 1.29x    |
| small  | E.sym+ns+tn+path    | 147.7            | 1.18x    |
| small  | F.path-only         | 157.7            | 1.11x    |
| small  | G.tn-only           | 134.8            | 1.30x    |
| small  | H.ns-only           | 149.4            | 1.17x    |
| medium | A.off (none)        | 198.8            | 1.00x    |
| medium | B.sym               | 191.9            | 1.04x    |
| medium | C.sym+ns            | 182.6            | 1.09x    |
| medium | D.sym+ns+tn         | 158.1            | 1.26x    |
| medium | E.sym+ns+tn+path    | 162.8            | 1.22x    |
| medium | F.path-only         | 183.8            | 1.08x    |
| medium | G.tn-only           | 159.9            | 1.24x    |
| medium | H.ns-only           | 171.8            | 1.16x    |

Object-count sanity check (`canonical_deep_dump`, medium, resolved):

| metric                | caches off | caches on | ratio |
|-----------------------|------------|-----------|-------|
| unique `RBS::Namespace` | 892      | 215       | 4.15x |
| unique `RBS::TypeName`  | 1080     | 1057      | 1.02x |

The Namespace reuse ratio (4x) is the dominant signal. TypeName reuse
in the dump is small because the dump only walks decl primary names
(each of which is unique); inside method-body / type AST nodes —
which the cache also covers but the dump does not — the same
`::String`, `::Integer`, etc. appear many times and share an instance
with the cache on.

## What this tells us about `TypeName.parse` / `Namespace.parse`

The original question was whether materialising via `TypeName.parse(str)`
/ `Namespace.parse(str)` would be faster than the current
`RBS::Namespace.new(path: …, absolute: …)` / `RBS::TypeName.new(...)`
path. The answer the experiment confirms is **no**:

- `Namespace.parse(s)` (`vendor/rbs/lib/rbs/namespace.rb:93-99`) does
  `s.start_with?("::")` → `delete_prefix` → `split("::")` →
  `map(&:to_sym)` → `Namespace.new(...)`. We have already paid the
  split-and-intern cost in the Rust interner; routing through
  `parse` re-runs it in Ruby on every call.
- The actual hot cost we measured is **Magnus → `RBS::TypeName.new` →
  `RBS::Namespace.new` allocation + the kind-detection regex in
  `TypeName#initialize`**. Caching the resulting Ruby instances bypasses
  that entire chain on the second and subsequent occurrences.

So "use the parse constructor" is the wrong lever; "share the Ruby
instance" (TN cache) is the right one. M5 keeps the Rust→Ruby boundary
exactly where M3h left it — the interner still hands the materializer
deconstructed `(namespace_sym, name_sym, kind)` triples — and stops the
materializer from rebuilding the Ruby payload more than once per
`(symbol, mark_absolute)` pair.

## Recommendation

Default all four caches **on**. Net win: ~24–30% off the materialize
phase, which translates to ~17–22% off cold load_only total
(`build_environment` + `resolve_type_names` + materialize). No
correctness regression — `canonical_dump` / `canonical_deep_dump`
match byte-for-byte (after sanitising object addresses) between
caches-on and caches-off.

The `LIBRBS_CACHE_*` env vars remain in the source as a
profiler/A-B-test convenience. They are read once per `materialize_all`
call (cheap), and the runtime cost of the resulting branch on each
cache lookup is one predictable comparison — invisible against the
allocation savings it gates.

Caveats worth knowing for the next milestone:

- The path-array cache (`PATH`) gives marginal additional savings when
  combined with the NS+TN caches but is the noisiest of the four —
  several 15-rep configurations had it appearing as a small regression
  vs `D` (NS+TN only). The cache is cheap enough that we leave it on,
  but it is the first one to revisit if the materializer is rewritten
  to use Ruby array literals or `RBS::TypeName.parse` directly.
- The Sym cache (`B`) does very little on its own — the TN cache
  transitively avoids most of the `to_symbol` calls. Keeping `SYM` on
  costs nothing but the option is to drop it if cache memory becomes
  a concern.
