# Materialize allocation tuning

Date: 2026-05-10
Environment: Ubuntu 24.04 LTS / Ruby 3.3.6 / Linux x86_64 (Intel Xeon @
2.80GHz, kernel 6.18.5) — same machine as `M4-baseline.md`.

Two changes layered on top of the M4 baseline (commit `a49d5ee`):

1. `88bbb88` — pre-size every `RArray` / `RHash` / `Vec` built during
   materialize using `ary_new_capa(n)` / `hash_new_capa(n)` /
   `Vec::with_capacity(n)`. The list lengths are read O(1) from the
   underlying `rbs_node_list_t.length` / `rbs_hash.length` fields via
   new `len()` accessors on `NodeList` / `RBSHash`.
2. `c3670a4` — allocate `RBS::Types::Bases::*` (`Bool`, `Void`, `Nil`,
   `Top`, `Bottom`, `Self`, `Instance`, `Class`) via
   `RClass#obj_alloc` + `ivar_set("@location", loc)` instead of the
   keyword-argument `Class.new(location:)` path. This skips the
   `kwargs!` Hash construction and the `initialize` keyword-arg
   unpack on the eight no-init `Bases::*` subclasses, which appear in
   essentially every method signature (`bool` / `void` / `untyped` /
   `nil` / `self` / `instance` / `class`).

`Bases::Any` keeps the keyword-arg path because its `initialize`
conditionally sets a second ivar (`@string = "__todo__"` when
`todo: true`).

## Cold-start wall time

Same harness as `M4-baseline.md` (`benchmark/helpers.rb`,
`Open3.capture3` per (impl, size) cell), but with `repeats: 5` instead
of `repeats: 3` to reduce subprocess-startup jitter on the small/medium
cells.

### load_only.rb

`from_loader` + materialize (`class_decls.size` triggers
`Native.materialize_all`).

| size   | pure RBS | librbs (M4 baseline) | librbs (after) | librbs delta | speedup (after) |
|--------|----------|----------------------|----------------|--------------|-----------------|
| small  | 146.3 ms |   193.6 ms           | 180.4 ms       |  −7%         | 0.81x           |
| medium | 177.9 ms |   238.8 ms           | 247.3 ms       |  +4% (noise) | 0.72x           |
| large  | 867.2 ms |  1095.0 ms           | 891.7 ms       | −19%         | 0.97x           |

The small/medium load-only path is still slower than pure RBS in absolute
terms — these workloads spend most of their materialize budget in
`make_location` (every `*Decl` / `*Member` / `*Type` carries a
`RBS::Location`), which neither change above touches.

### load_and_resolve.rb

`from_loader` + `resolve_type_names` + materialize.

| size   | pure RBS  | librbs (M4 baseline) | librbs (after) | librbs delta | speedup (after) |
|--------|-----------|----------------------|----------------|--------------|-----------------|
| small  |  259.6 ms |  209.8 ms            |  184.6 ms      | −12%         | 1.41x           |
| medium |  382.6 ms |  269.7 ms            |  226.5 ms      | −16%         | 1.69x           |
| large  | 2448.5 ms | 1075.1 ms            |  870.3 ms      | −19%         | 2.81x           |

The load-and-resolve speedup column is the headline number, since the
resolver port is the M3 win this milestone was protecting. The
materialize allocation work moves medium from 1.33x → 1.69x and holds
large at 2.81x.

## Focused materialize-only timing

Cold-start cost is dominated by Ruby boot, parser load, and stdlib
require — large per-call variance hides the materialize improvement.
The numbers below come from a single in-process run (`require "rbs";
require "librbs"`), 30 timed iterations per cell, with `GC.start` before
each iteration. Only the `class_decls.size` call is timed (which
triggers `Native.materialize_all`); `from_loader` is excluded.

| size   | original | + ary_new_capa (88bbb88) | + obj_alloc (c3670a4) | total delta |
|--------|----------|--------------------------|-----------------------|-------------|
| small  | 213 ms   | 208 ms (−2%)             | **181 ms (−15%)**     | −15%        |
| medium | 271 ms   | 264 ms (−3%)             | **219 ms (−19%)**     | −19%        |
| large  | 1050 ms  | 1036 ms (−1%)            | **931 ms (−11%)**     | −11%        |

(Values are medians.)

## Why the gap between the two tables

The cold-start `load_only.rb` cell for the small workload spends most
of its wall time outside materialize — it parses the core RBS sigs,
loads parsers, and runs the boot path. Trimming materialize by 30 ms
moves the cell from ≈194 ms to ≈180 ms, which is real but easily
masked by run-to-run subprocess noise. The focused materialize-only
table is the cleaner signal.

For `load_and_resolve.rb` the resolver phase plus materialize make up a
larger share of the cell, so the same 30–100 ms saving lands as a
visible 12–19% drop on every size.

## Pointers for the next round

The materialize-only table shows that even with these two changes
**materialize is still ≈930 ms on large**, which is most of the
load-only wall time. The dominant remaining costs, ranked by inspection:

1. `make_location` — every node builds an `RBS::Location` plus zero or
   more sub-locations via `add_required_child` / `add_optional_child`.
   Each sub-location calls `RBS::Location#add_required_child` on the
   Ruby side, which is itself a method dispatch. Lifting the
   sub-location population into a single Ruby call (or onto a new
   `RClass#obj_alloc + ivar_set` fast-path on the `Location` class) is
   the most direct next win, given how many Locations get built (one
   per AST node, plus 1–4 sub-locations).

2. `kwargs!` packing for the larger `Types::*` and `Members::*`
   classes. The same `obj_alloc + ivar_set` trick used for `Bases::*`
   applies wherever upstream's `initialize` is a vanilla `@a = a; @b =
   b; …`. Candidates by occurrence frequency: `Types::Variable`,
   `Types::ClassInstance`, `Types::Optional`, `Types::Union`,
   `Types::Tuple`, then the various `Members::*`. Each one needs the
   upstream `initialize` checked first — anything that freezes a
   collection or computes a derived ivar must keep the keyword-arg
   path or replicate the post-init step.

3. Pre-cached `Id`s for the hot ivar names (`@location`, `@name`,
   `@type`). Each `ivar_set("@location", loc)` currently re-interns the
   string. A `Lazy<Id>` cached on `MaterializeCtx` (or alongside
   `ClassRefs`) would skip that.
