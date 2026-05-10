//! Lightweight phase timing for `materialize_all`.
//!
//! Wrap a hot fn body with `let _t = PhaseTimer::new(Phase::Type);`.
//! `PhaseTimer` deducts child phases' wall time from its own on drop,
//! so each phase's reported time is *exclusive self-time* — phases at
//! different nesting levels can be summed without double-counting.
//!
//! Cost: 2 × `Instant::now()` per call (~50–100 ns) plus a TLS swap.
//! Avoid wrapping leaf-most hot fns (e.g. `materialize_namespace`)
//! that get called millions of times.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

#[repr(usize)]
#[derive(Copy, Clone, Debug)]
#[allow(dead_code)]
pub enum Phase {
    SourceRbs = 0,
    Directives,
    Declaration,
    NestedDecl,
    Member,
    MethodType,
    Type,
    Location,
    TypeParam,
    AddSource,
    _Last,
}

const N: usize = Phase::_Last as usize;

static NANOS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static CALLS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

const NAMES: [&str; N] = [
    "source_rbs",
    "directives",
    "declaration",
    "nested_decl",
    "member",
    "method_type",
    "type",
    "location",
    "type_param",
    "ruby_add_source",
];

thread_local! {
    /// Sum (ns) of all currently-finished child timers belonging to
    /// the in-flight parent timer. Drained on parent drop so the
    /// parent reports exclusive self-time.
    static CHILD_NS: Cell<u64> = const { Cell::new(0) };
}

pub struct PhaseTimer {
    phase: Phase,
    start: Instant,
    saved_child_ns: u64,
}

impl PhaseTimer {
    #[inline]
    pub fn new(phase: Phase) -> Self {
        let saved = CHILD_NS.with(|c| c.replace(0));
        Self {
            phase,
            start: Instant::now(),
            saved_child_ns: saved,
        }
    }
}

impl Drop for PhaseTimer {
    #[inline]
    fn drop(&mut self) {
        let elapsed = self.start.elapsed().as_nanos() as u64;
        let consumed_by_children = CHILD_NS.with(|c| c.get());
        let self_ns = elapsed.saturating_sub(consumed_by_children);
        let i = self.phase as usize;
        NANOS[i].fetch_add(self_ns, Ordering::Relaxed);
        CALLS[i].fetch_add(1, Ordering::Relaxed);
        // Contribute *full* elapsed time (incl. our children) to our
        // parent's child-budget, so the parent's self-time excludes us.
        CHILD_NS.with(|c| c.set(self.saved_child_ns + elapsed));
    }
}

pub fn dump_and_reset() -> String {
    let mut rows: Vec<(&'static str, u64, u64)> = (0..N)
        .map(|i| {
            (
                NAMES[i],
                NANOS[i].swap(0, Ordering::Relaxed),
                CALLS[i].swap(0, Ordering::Relaxed),
            )
        })
        .collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1));
    let total: u64 = rows.iter().map(|(_, ns, _)| *ns).sum();
    let total_ms = (total as f64) / 1_000_000.0;

    let mut out = String::new();
    out.push_str("== materialize phase self-time ==\n");
    out.push_str(&format!(
        "  {:<18} {:>12} {:>12} {:>10} {:>14}\n",
        "phase", "self ms", "calls", "% total", "ns / call"
    ));
    out.push_str(&format!("  {:-<70}\n", ""));
    for (name, ns, calls) in &rows {
        if *calls == 0 {
            continue;
        }
        let ms = (*ns as f64) / 1_000_000.0;
        let pct = if total > 0 {
            100.0 * (*ns as f64) / (total as f64)
        } else {
            0.0
        };
        let per = if *calls > 0 { *ns / *calls } else { 0 };
        out.push_str(&format!(
            "  {:<18} {:>12.2} {:>12} {:>9.1}% {:>14}\n",
            name, ms, calls, pct, per
        ));
    }
    out.push_str(&format!(
        "  {:-<70}\n  {:<18} {:>12.2}\n",
        "", "total self-time", total_ms
    ));
    out
}
