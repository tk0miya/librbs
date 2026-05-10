//! Per-decl `Resolution` side-table.
//!
//! For every type-name occurrence the resolver visits inside a single
//! declaration, the resolved (or unresolved) target is pushed onto a
//! `Vec<ResolvedRef>` keyed by that declaration's [`DeclRef`]. The
//! per-decl ordering matches the resolver's pre-order walk — the same
//! order the materializer (M3f–M3h) walks each decl's subtree — so the
//! materializer can consume resolutions one-by-one as it encounters
//! type-name nodes, without needing a positional ID on each AST node.
//!
//! This replaces the earlier `(source_index, serial)`-keyed `NodeId`
//! scheme: keying on `DeclRef` lets the resolver and the materializer
//! agree on per-decl pre-order without a positional ID on each AST node.

use rustc_hash::FxHashMap;

use crate::env::entry::DeclRef;
use crate::interner::TypeNameSym;

/// One resolution outcome. `Resolved` is the absolute `TypeNameSym` the
/// occurrence binds to. `Unresolved` carries the *original* (pre-resolve)
/// name back unchanged so downstream consumers can still report a sensible
/// diagnostic without looking the AST node up again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedRef {
    Resolved(TypeNameSym),
    Unresolved(TypeNameSym),
}

#[derive(Debug, Default)]
pub struct Resolution {
    /// Resolved type-name occurrences for each declaration, in the
    /// resolver's pre-order. Private — callers go through [`record`],
    /// [`get`], [`iter`], [`len`], [`is_empty`].
    entries: FxHashMap<DeclRef, Vec<ResolvedRef>>,
}

impl Resolution {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one resolution outcome for `decl_ref`. The resolver
    /// records once per type-name occurrence in pre-order; the
    /// materializer reads the recorded slice back via [`get`] in the
    /// same order.
    pub fn record(&mut self, decl_ref: DeclRef, resolved: ResolvedRef) {
        self.entries.entry(decl_ref).or_default().push(resolved);
    }

    /// Look up the recorded resolution slice for `decl_ref`. Returns
    /// `None` when the decl had no type-name occurrences (nothing was
    /// recorded) or was skipped during resolution (`only:` filter, or
    /// the `# resolve-type-names: false` magic comment short-circuited
    /// the whole source).
    pub fn get(&self, decl_ref: DeclRef) -> Option<&[ResolvedRef]> {
        self.entries.get(&decl_ref).map(Vec::as_slice)
    }

    /// Iterate over every recorded `(DeclRef, &[ResolvedRef])` pair.
    /// Order is unspecified (the underlying map is hashed).
    pub fn iter(&self) -> impl Iterator<Item = (DeclRef, &[ResolvedRef])> + '_ {
        self.entries.iter().map(|(k, v)| (*k, v.as_slice()))
    }

    /// Total resolution count across all decls. Mostly useful for
    /// tests; the materializer never needs the global count.
    pub fn len(&self) -> usize {
        self.entries.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interner::TypeNameSym;

    fn dr(source_index: u32, decl_index: u32) -> DeclRef {
        DeclRef {
            source_index,
            decl_index,
        }
    }

    #[test]
    fn record_appends_in_call_order() {
        let mut r = Resolution::new();
        r.record(dr(0, 0), ResolvedRef::Resolved(TypeNameSym(1)));
        r.record(dr(0, 0), ResolvedRef::Unresolved(TypeNameSym(2)));
        let slice = r.get(dr(0, 0)).unwrap();
        assert_eq!(slice.len(), 2);
        assert_eq!(slice[0], ResolvedRef::Resolved(TypeNameSym(1)));
        assert_eq!(slice[1], ResolvedRef::Unresolved(TypeNameSym(2)));
    }

    #[test]
    fn get_returns_none_for_unrecorded_decl_ref() {
        let mut r = Resolution::new();
        r.record(dr(0, 0), ResolvedRef::Resolved(TypeNameSym(1)));
        assert!(r.get(dr(0, 1)).is_none());
        assert!(r.get(dr(1, 0)).is_none());
    }

    #[test]
    fn len_sums_across_decls() {
        let mut r = Resolution::new();
        r.record(dr(0, 0), ResolvedRef::Resolved(TypeNameSym(1)));
        r.record(dr(0, 1), ResolvedRef::Resolved(TypeNameSym(2)));
        r.record(dr(0, 1), ResolvedRef::Resolved(TypeNameSym(3)));
        assert_eq!(r.len(), 3);
        assert!(!r.is_empty());
    }

    #[test]
    fn empty_resolution_is_empty_and_zero_len() {
        let r = Resolution::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }
}
