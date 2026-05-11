//! Per-materialize-run caches for Ruby objects keyed by interner symbols.
//!
//! The materializer rebuilds the same `RBS::TypeName`, `RBS::Namespace`,
//! and Ruby `Symbol` values thousands of times across an stdlib-sized
//! environment — every occurrence of `String`, `Integer`, etc. costs one
//! `RBS::TypeName.new(...)` plus one `RBS::Namespace.new(...)` plus N
//! `to_symbol(...)` calls in the current `materialize/type_name.rs` path.
//!
//! These objects are immutable and equality-based on upstream (see
//! `vendor/rbs/lib/rbs/type_name.rb` / `namespace.rb`), so a single Ruby
//! instance can be shared across every occurrence. The caches below hold
//! that shared instance for the lifetime of one `materialize_all` call.
//!
//! Values stored:
//!
//! - Ruby `Symbol` values are safe to retain because they are either
//!   static (pinned, never GC'd) or, even when dynamic, kept alive by
//!   Ruby's symbol table for the symbol's lifetime — and `materialize_all`
//!   holds the GVL the entire time it runs, so no GC sweep can run
//!   between cache insertion and last use.
//! - Ruby `RBS::Namespace` / `RBS::TypeName` instances are reachable from
//!   the partially-built environment (each `add_source` call installs
//!   them into `@class_decls` / `@sources`) for the duration of
//!   `materialize_all`. We also publish each cached value through a
//!   funcall return into Ruby, so the GC sees it transitively.

use librbs_core::interner::{NamespaceSym, Sym, TypeNameSym};
use magnus::Value;

/// Per-cache enable flags read once from env vars at materialize start.
/// Lets the experiment harness A/B individual caches.
#[derive(Debug, Clone, Copy)]
pub struct CacheFlags {
    pub sym: bool,
    pub ns: bool,
    pub tn: bool,
    pub path: bool,
}

impl CacheFlags {
    pub fn from_env() -> Self {
        fn on(var: &str) -> bool {
            match std::env::var(var) {
                Err(_) => true,
                Ok(v) => !matches!(v.as_str(), "0" | "off" | "false"),
            }
        }
        Self {
            sym: on("LIBRBS_CACHE_SYM"),
            ns: on("LIBRBS_CACHE_NS"),
            tn: on("LIBRBS_CACHE_TN"),
            path: on("LIBRBS_CACHE_PATH"),
        }
    }

}

/// Cache the Ruby `Symbol` Value that corresponds to each interner
/// [`Sym`]. The interner already deduplicates the string, so the cache is
/// a `Vec<Option<Value>>` indexed by `Sym.0` — dense, zero hashing.
#[derive(Default)]
pub struct SymbolCache {
    slots: Vec<Option<Value>>,
}

impl SymbolCache {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            slots: vec![None; n],
        }
    }

    /// Look up a cached Ruby `Symbol` Value. Returns `None` on a miss;
    /// callers compute the Symbol and call [`Self::insert`].
    #[inline]
    pub fn get(&self, sym: Sym) -> Option<Value> {
        self.slots.get(sym.0 as usize).copied().flatten()
    }

    #[inline]
    pub fn insert(&mut self, sym: Sym, value: Value) {
        let idx = sym.0 as usize;
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, None);
        }
        self.slots[idx] = Some(value);
    }
}

/// Cache the materialised `RBS::Namespace` Ruby Value per
/// `(NamespaceSym, mark_absolute)`. The `mark_absolute` axis is needed
/// because `build_type_name_from_sym` may materialise a relative interner
/// namespace as an absolute Ruby namespace (the
/// `Resolved(_)` branch in `materialize_resolved_type_name`).
#[derive(Default)]
pub struct NamespaceCache {
    /// Stored back-to-back: index `2*i` holds the relative materialisation
    /// of `NamespaceSym(i)`, `2*i+1` holds the absolute one. Both are
    /// `None` until first use.
    slots: Vec<Option<Value>>,
}

impl NamespaceCache {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            slots: vec![None; n.saturating_mul(2)],
        }
    }

    #[inline]
    fn slot_index(sym: NamespaceSym, absolute: bool) -> usize {
        (sym.0 as usize) * 2 + (absolute as usize)
    }

    #[inline]
    pub fn get(&self, sym: NamespaceSym, absolute: bool) -> Option<Value> {
        self.slots
            .get(Self::slot_index(sym, absolute))
            .copied()
            .flatten()
    }

    #[inline]
    pub fn insert(&mut self, sym: NamespaceSym, absolute: bool, value: Value) {
        let idx = Self::slot_index(sym, absolute);
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, None);
        }
        self.slots[idx] = Some(value);
    }
}

/// Cache the materialised `RBS::TypeName` Ruby Value per
/// `(TypeNameSym, mark_absolute)`. See [`NamespaceCache`] for the
/// `mark_absolute` axis rationale.
#[derive(Default)]
pub struct TypeNameCache {
    slots: Vec<Option<Value>>,
}

impl TypeNameCache {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            slots: vec![None; n.saturating_mul(2)],
        }
    }

    #[inline]
    fn slot_index(sym: TypeNameSym, absolute: bool) -> usize {
        (sym.0 as usize) * 2 + (absolute as usize)
    }

    #[inline]
    pub fn get(&self, sym: TypeNameSym, absolute: bool) -> Option<Value> {
        self.slots
            .get(Self::slot_index(sym, absolute))
            .copied()
            .flatten()
    }

    #[inline]
    pub fn insert(&mut self, sym: TypeNameSym, absolute: bool, value: Value) {
        let idx = Self::slot_index(sym, absolute);
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, None);
        }
        self.slots[idx] = Some(value);
    }
}

/// Cache the Ruby `Array<Symbol>` (the `path:` kwarg for
/// `RBS::Namespace.new`) per `NamespaceSym`. The array contents only
/// depend on the symbol path, not on the `absolute` flag, so this is
/// one slot per `NamespaceSym`.
#[derive(Default)]
pub struct PathArrayCache {
    slots: Vec<Option<Value>>,
}

impl PathArrayCache {
    pub fn with_capacity(n: usize) -> Self {
        Self {
            slots: vec![None; n],
        }
    }

    #[inline]
    pub fn get(&self, sym: NamespaceSym) -> Option<Value> {
        self.slots.get(sym.0 as usize).copied().flatten()
    }

    #[inline]
    pub fn insert(&mut self, sym: NamespaceSym, value: Value) {
        let idx = sym.0 as usize;
        if idx >= self.slots.len() {
            self.slots.resize(idx + 1, None);
        }
        self.slots[idx] = Some(value);
    }
}
