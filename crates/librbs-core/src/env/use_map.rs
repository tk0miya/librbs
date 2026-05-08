//! Port of `RBS::Environment::UseMap` from the upstream Ruby implementation.
//!
//! The two-stage shape mirrors upstream: a [`Table`] is a global index of
//! known type names (with a parent-namespace → child-types reverse map),
//! and a [`UseMap`] is an alias map built from `# use ...` directives that
//! resolves relative names to absolute ones using the table.
//!
//! The directive-walking code that converts a parsed `# use ...` clause
//! into [`UseMap::add_single`] / [`UseMap::add_wildcard`] calls is part of
//! the M3b driver — it lives with the AST traversal.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::env::Environment;
use crate::interner::{NamespaceSym, Sym, TypeNameInterner, TypeNameSym};

/// Global name index shared across all `UseMap`s in an environment.
///
/// `known_types` is the union of every declared `TypeName` (class/module,
/// interface, type alias). `children` is the reverse index: namespace →
/// the set of names directly underneath it. Both store absolute names.
#[derive(Debug, Default)]
pub struct Table {
    pub known_types: FxHashSet<TypeNameSym>,
    pub children: FxHashMap<NamespaceSym, FxHashSet<TypeNameSym>>,
}

impl Table {
    pub fn new() -> Self {
        Self::default()
    }

    /// Populate `known_types` from `env` (class, interface, type alias
    /// decls). Mirrors the upstream behavior of seeding the table from the
    /// environment before any UseMaps are constructed.
    pub fn populate_from(&mut self, env: &Environment) {
        self.known_types.extend(env.class_decls.keys().copied());
        self.known_types.extend(env.interface_decls.keys().copied());
        self.known_types
            .extend(env.type_alias_decls.keys().copied());
    }

    /// Compute the namespace → child-types reverse index. Must be called
    /// after `populate_from`. Clears the existing `children` map first to
    /// match upstream's `compute_children` behavior.
    pub fn compute_children(&mut self, interner: &TypeNameInterner) {
        self.children.clear();
        for &tn in &self.known_types {
            let ns = interner.namespace_of(tn);
            if interner.namespaces.is_empty(ns) {
                continue;
            }
            self.children.entry(ns).or_default().insert(tn);
        }
    }
}

/// Alias map built from a collection of `# use ...` directives.
///
/// `map` resolves an unqualified head segment (`Sym`) to the absolute
/// `TypeNameSym` it should be rewritten to.
#[derive(Debug)]
pub struct UseMap<'t> {
    table: &'t Table,
    map: FxHashMap<Sym, TypeNameSym>,
}

impl<'t> UseMap<'t> {
    pub fn new(table: &'t Table) -> Self {
        Self {
            table,
            map: FxHashMap::default(),
        }
    }

    /// Implements the `SingleClause` branch of upstream's `build_map`.
    /// `type_name` should already be resolved to an absolute name; the
    /// caller is responsible for the `absolute!` step (mirrors upstream).
    pub fn add_single(
        &mut self,
        type_name: TypeNameSym,
        new_name: Option<Sym>,
        interner: &TypeNameInterner,
    ) {
        let key = new_name.unwrap_or_else(|| interner.name_of(type_name));
        self.map.insert(key, type_name);
    }

    /// Implements the `WildcardClause` branch of upstream's `build_map`.
    /// All children of `namespace` (which must be absolute) are bound by
    /// their last-segment name.
    pub fn add_wildcard(&mut self, namespace: NamespaceSym, interner: &TypeNameInterner) {
        if let Some(children) = self.table.children.get(&namespace) {
            for &child in children {
                let name = interner.name_of(child);
                self.map.insert(name, child);
            }
        }
    }

    /// `RBS::Environment::UseMap#resolve?` — returns `Some(absolute)` if
    /// the head segment of an *unqualified* `type_name` is mapped, else
    /// `None`. Absolute names pass through unchanged.
    pub fn resolve_opt(
        &self,
        type_name: TypeNameSym,
        interner: &mut TypeNameInterner,
    ) -> Option<TypeNameSym> {
        let (ns, name, kind) = interner.lookup(type_name);
        let (path, absolute) = interner.namespaces.lookup(ns).clone();
        if absolute {
            return None;
        }

        if let Some((&head, tail)) = path.split_first() {
            // Namespace is non-empty: rewrite head via the map, keep tail.
            let mapped = *self.map.get(&head)?;
            let (mapped_ns, mapped_name, _) = interner.lookup(mapped);
            let mapped_path = interner.namespaces.lookup(mapped_ns).0.clone();
            let mut new_path = mapped_path;
            new_path.push(mapped_name);
            new_path.extend_from_slice(tail);
            let new_ns = interner.namespaces.intern(&new_path, true);
            Some(interner.intern(new_ns, name, kind))
        } else {
            // Empty namespace: rewrite the leaf itself.
            self.map.get(&name).copied()
        }
    }

    /// `RBS::Environment::UseMap#resolve` — `resolve_opt`, falling back to
    /// the input on miss.
    pub fn resolve(&self, type_name: TypeNameSym, interner: &mut TypeNameInterner) -> TypeNameSym {
        self.resolve_opt(type_name, interner).unwrap_or(type_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interner::{TypeNameInterner, TypeNameKind};

    fn intern_type(
        interner: &mut TypeNameInterner,
        absolute: bool,
        path: &[&str],
        name: &str,
    ) -> TypeNameSym {
        let segs: Vec<Sym> = path.iter().map(|s| interner.symbols.intern(s)).collect();
        let ns = interner.namespaces.intern(&segs, absolute);
        let name_sym = interner.symbols.intern(name);
        let kind = TypeNameKind::detect(name);
        interner.intern(ns, name_sym, kind)
    }

    #[test]
    fn single_clause_with_rename_maps_alias_to_absolute() {
        let mut interner = TypeNameInterner::new();
        let foo_bar = intern_type(&mut interner, true, &["Foo"], "Bar");
        let table = Table::new();
        let mut map = UseMap::new(&table);
        let alias = interner.symbols.intern("MyBar");
        map.add_single(foo_bar, Some(alias), &interner);

        // Unqualified relative `MyBar` should now rewrite to `::Foo::Bar`.
        let probe = intern_type(&mut interner, false, &[], "MyBar");
        let resolved = map.resolve(probe, &mut interner);
        assert_eq!(resolved, foo_bar);
    }

    #[test]
    fn single_clause_without_rename_maps_by_last_name() {
        let mut interner = TypeNameInterner::new();
        let foo_bar = intern_type(&mut interner, true, &["Foo"], "Bar");
        let table = Table::new();
        let mut map = UseMap::new(&table);
        map.add_single(foo_bar, None, &interner);

        let probe = intern_type(&mut interner, false, &[], "Bar");
        assert_eq!(map.resolve(probe, &mut interner), foo_bar);
    }

    #[test]
    fn nonempty_namespace_rewrites_head_only() {
        let mut interner = TypeNameInterner::new();
        let foo_bar = intern_type(&mut interner, true, &["Foo"], "Bar");
        let foo_bar_baz = intern_type(&mut interner, true, &["Foo", "Bar"], "Baz");
        let table = Table::new();
        let mut map = UseMap::new(&table);
        map.add_single(foo_bar, None, &interner);

        // `Bar::Baz` (relative) should rewrite the head `Bar` and keep `Baz`.
        let probe = intern_type(&mut interner, false, &["Bar"], "Baz");
        assert_eq!(map.resolve(probe, &mut interner), foo_bar_baz);
    }

    #[test]
    fn absolute_input_passes_through() {
        let mut interner = TypeNameInterner::new();
        let foo_bar = intern_type(&mut interner, true, &["Foo"], "Bar");
        let table = Table::new();
        let mut map = UseMap::new(&table);
        let alias = interner.symbols.intern("Bar");
        map.add_single(foo_bar, Some(alias), &interner);

        let probe = intern_type(&mut interner, true, &[], "Bar"); // ::Bar
        let resolved = map.resolve(probe, &mut interner);
        assert_eq!(resolved, probe);
    }

    #[test]
    fn table_children_lookup_and_wildcard_clause() {
        // Set up a Table containing ::Foo::Bar and ::Foo::Baz, then build a
        // UseMap with a wildcard that imports everything under ::Foo.
        let mut interner = TypeNameInterner::new();
        let foo_bar = intern_type(&mut interner, true, &["Foo"], "Bar");
        let foo_baz = intern_type(&mut interner, true, &["Foo"], "Baz");
        let foo_ns = interner
            .namespaces
            .intern(&[interner.symbols.intern("Foo")], true);

        let mut table = Table::new();
        table.known_types.insert(foo_bar);
        table.known_types.insert(foo_baz);
        table.compute_children(&interner);

        let children = table.children.get(&foo_ns).expect("Foo has children");
        assert!(children.contains(&foo_bar));
        assert!(children.contains(&foo_baz));

        let mut map = UseMap::new(&table);
        map.add_wildcard(foo_ns, &interner);

        let probe_bar = intern_type(&mut interner, false, &[], "Bar");
        let probe_baz = intern_type(&mut interner, false, &[], "Baz");
        assert_eq!(map.resolve(probe_bar, &mut interner), foo_bar);
        assert_eq!(map.resolve(probe_baz, &mut interner), foo_baz);
    }
}
