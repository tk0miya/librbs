//! `Resolution` is the side-table that records, for every type-name
//! occurrence in the AST, which fully-qualified name it resolves to.
//!
//! Built per-source by the M3b driver and merged into a single map at the
//! end. Keeping the table outside the AST lets us run resolution in
//! parallel without needing a `&mut` to ruby-rbs's parser-owned tree.

use rustc_hash::FxHashMap;

use crate::interner::TypeNameSym;

/// Identifies one type-name occurrence in the parsed AST.
///
/// `serial` is assigned by a deterministic pre-order walk of each source's
/// declarations — the same walk in different runs produces the same
/// numbers. Two occurrences in different sources never collide because
/// `source_index` is included.
///
/// `serial` was chosen over the parsed AST's byte offset because
/// `ruby-rbs` does not currently expose a stable per-node offset; if that
/// changes the underlying field can be swapped without breaking callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId {
    pub source_index: u32,
    pub serial: u32,
}

impl NodeId {
    pub fn new(source_index: u32, serial: u32) -> Self {
        Self {
            source_index,
            serial,
        }
    }
}

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
    pub type_name_resolutions: FxHashMap<NodeId, ResolvedRef>,
}

impl Resolution {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, id: NodeId, resolved: ResolvedRef) {
        debug_assert!(
            !self.type_name_resolutions.contains_key(&id),
            "duplicate NodeId in Resolution: {:?}",
            id
        );
        self.type_name_resolutions.insert(id, resolved);
    }

    /// Merge another `Resolution` into this one. Used by the M3b driver to
    /// fold per-source results back together. Panics in debug builds if a
    /// `NodeId` appears in both — that would indicate a bug in NodeId
    /// allocation rather than a legitimate overlap.
    pub fn merge(&mut self, other: Resolution) {
        for (id, resolved) in other.type_name_resolutions {
            debug_assert!(
                !self.type_name_resolutions.contains_key(&id),
                "duplicate NodeId during Resolution::merge: {:?}",
                id
            );
            self.type_name_resolutions.insert(id, resolved);
        }
    }

    pub fn len(&self) -> usize {
        self.type_name_resolutions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.type_name_resolutions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interner::TypeNameSym;

    #[test]
    fn merge_combines_disjoint_resolutions() {
        let mut a = Resolution::new();
        let mut b = Resolution::new();
        a.record(NodeId::new(0, 0), ResolvedRef::Resolved(TypeNameSym(1)));
        b.record(NodeId::new(0, 1), ResolvedRef::Unresolved(TypeNameSym(2)));
        a.merge(b);
        assert_eq!(a.len(), 2);
    }

    #[test]
    #[should_panic(expected = "duplicate NodeId")]
    fn merge_panics_on_duplicate_node_id_in_debug() {
        let mut a = Resolution::new();
        let mut b = Resolution::new();
        let id = NodeId::new(0, 0);
        a.record(id, ResolvedRef::Resolved(TypeNameSym(1)));
        b.record(id, ResolvedRef::Resolved(TypeNameSym(2)));
        a.merge(b);
    }
}
