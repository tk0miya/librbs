use std::collections::HashMap;

/// Interned string symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sym(pub u32);

/// Interned namespace = `(path, absolute)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NamespaceSym(pub u32);

/// Interned `TypeName`. Hash-consed over `(namespace, name, kind)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TypeNameSym(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeNameKind {
    Class,
    Interface,
    TypeAlias,
}

impl TypeNameKind {
    /// Mirror RBS::TypeName kind detection.
    pub fn detect(name: &str) -> Self {
        let first = name.chars().next();
        match first {
            Some(c) if c.is_ascii_uppercase() => TypeNameKind::Class,
            Some('_') => TypeNameKind::Interface,
            Some(c) if c.is_ascii_lowercase() => TypeNameKind::TypeAlias,
            _ => TypeNameKind::Class,
        }
    }
}

// ---------- SymbolInterner ----------

#[derive(Debug, Default)]
pub struct SymbolInterner {
    map: HashMap<String, Sym>,
    rev: Vec<String>,
}

impl SymbolInterner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = Sym(self.rev.len() as u32);
        self.rev.push(s.to_owned());
        self.map.insert(s.to_owned(), sym);
        sym
    }

    pub fn lookup(&self, sym: Sym) -> &str {
        &self.rev[sym.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.rev.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rev.is_empty()
    }

    /// Read-only lookup by string. Exposed to consumers via
    /// [`FrozenSymbols::intern`].
    fn find(&self, s: &str) -> Option<Sym> {
        self.map.get(s).copied()
    }
}

// ---------- NamespaceInterner ----------

#[derive(Debug, Default)]
pub struct NamespaceInterner {
    map: HashMap<(Vec<Sym>, bool), NamespaceSym>,
    rev: Vec<(Vec<Sym>, bool)>,
}

impl NamespaceInterner {
    pub fn new() -> Self {
        let mut this = Self::default();
        // index 0 = relative empty
        this.intern_owned(Vec::new(), false);
        // index 1 = absolute root
        this.intern_owned(Vec::new(), true);
        this
    }

    fn intern_owned(&mut self, path: Vec<Sym>, absolute: bool) -> NamespaceSym {
        use std::collections::hash_map::Entry;
        match self.map.entry((path, absolute)) {
            Entry::Occupied(e) => *e.get(),
            Entry::Vacant(e) => {
                let ns = NamespaceSym(self.rev.len() as u32);
                self.rev.push(e.key().clone());
                e.insert(ns);
                ns
            }
        }
    }

    /// Intern a namespace from a borrowed slice. Allocates one `Vec<Sym>`
    /// per call (even on hit) because the `HashMap` key is owned. If
    /// profiling later shows this is hot, switch to `hashbrown`'s
    /// `raw_entry` API to avoid the allocation on hit.
    pub fn intern(&mut self, path: &[Sym], absolute: bool) -> NamespaceSym {
        self.intern_owned(path.to_vec(), absolute)
    }

    pub fn empty_relative(&self) -> NamespaceSym {
        NamespaceSym(0)
    }

    pub fn root_absolute(&self) -> NamespaceSym {
        NamespaceSym(1)
    }

    pub fn lookup(&self, ns: NamespaceSym) -> &(Vec<Sym>, bool) {
        &self.rev[ns.0 as usize]
    }

    /// `RBS::Namespace#append`: returns the namespace `ns` with `seg`
    /// appended at the end.
    pub fn append(&mut self, ns: NamespaceSym, seg: Sym) -> NamespaceSym {
        match self.append_lookup(ns, seg) {
            Ok(found) => found,
            Err((path, absolute)) => self.intern_owned(path, absolute),
        }
    }

    /// Join two namespaces with the same asymmetric rule as
    /// `std::path::Path::join` (and `RBS::Namespace#+`): if `rhs` is
    /// absolute, it replaces `lhs` entirely; otherwise the paths are
    /// concatenated and the result keeps `lhs`'s absolute flag.
    pub fn join(&mut self, lhs: NamespaceSym, rhs: NamespaceSym) -> NamespaceSym {
        match self.join_lookup(lhs, rhs) {
            Ok(found) => found,
            Err((path, absolute)) => self.intern_owned(path, absolute),
        }
    }

    /// `RBS::Namespace#empty?` — true when the path has no segments,
    /// regardless of the absolute flag.
    pub fn is_empty(&self, ns: NamespaceSym) -> bool {
        self.lookup(ns).0.is_empty()
    }

    /// `RBS::Namespace#relative?` — true when not absolute.
    pub fn is_relative(&self, ns: NamespaceSym) -> bool {
        !self.lookup(ns).1
    }

    /// `RBS::Namespace#parent` — drop the last segment, preserving
    /// `absolute`. Returns `None` for namespaces with no segments
    /// (the empty relative or root absolute have no parent).
    pub fn parent(&mut self, ns: NamespaceSym) -> Option<NamespaceSym> {
        let (path, absolute) = self.lookup(ns).clone();
        if path.is_empty() {
            return None;
        }
        let mut parent = path;
        parent.pop();
        Some(self.intern_owned(parent, absolute))
    }

    /// `RBS::Namespace#to_type_name` — split into `(parent_namespace, last_segment)`.
    /// Returns `None` for namespaces with no segments.
    pub fn to_type_name(&mut self, ns: NamespaceSym) -> Option<(NamespaceSym, Sym)> {
        let (path, absolute) = self.lookup(ns).clone();
        let last = *path.last()?;
        let mut parent = path;
        parent.pop();
        Some((self.intern_owned(parent, absolute), last))
    }

    // ---------- Read-only primitives ----------
    //
    // The `*_lookup` helpers below produce `Result<Sym, components>`:
    // `Ok` if the namespace already exists, `Err` carrying the path +
    // absolute flag otherwise. Mutating wrappers (`append`, `join`)
    // discard the `Err` payload through `intern_owned`; the read-only
    // [`FrozenNamespaces`] view discards `Err` through `Result::ok`.
    // Both paths share the same candidate construction.

    fn find(&self, path: &[Sym], absolute: bool) -> Option<NamespaceSym> {
        self.map.get(&(path.to_vec(), absolute)).copied()
    }

    fn append_lookup(&self, ns: NamespaceSym, seg: Sym) -> Result<NamespaceSym, (Vec<Sym>, bool)> {
        let (path, absolute) = self.lookup(ns);
        let mut new_path = path.clone();
        new_path.push(seg);
        let absolute = *absolute;
        match self.find(&new_path, absolute) {
            Some(found) => Ok(found),
            None => Err((new_path, absolute)),
        }
    }

    fn join_lookup(
        &self,
        lhs: NamespaceSym,
        rhs: NamespaceSym,
    ) -> Result<NamespaceSym, (Vec<Sym>, bool)> {
        let (rpath, rabs) = self.lookup(rhs);
        if *rabs {
            return Ok(rhs);
        }
        let (lpath, labs) = self.lookup(lhs);
        let mut path = lpath.clone();
        path.extend_from_slice(rpath);
        let absolute = *labs;
        match self.find(&path, absolute) {
            Some(found) => Ok(found),
            None => Err((path, absolute)),
        }
    }
}

// ---------- TypeNameInterner ----------

#[derive(Debug, Default)]
pub struct TypeNameInterner {
    pub symbols: SymbolInterner,
    pub namespaces: NamespaceInterner,
    map: HashMap<(NamespaceSym, Sym, TypeNameKind), TypeNameSym>,
    rev: Vec<(NamespaceSym, Sym, TypeNameKind)>,
}

impl TypeNameInterner {
    pub fn new() -> Self {
        Self {
            symbols: SymbolInterner::new(),
            namespaces: NamespaceInterner::new(),
            map: HashMap::new(),
            rev: Vec::new(),
        }
    }

    pub fn intern(
        &mut self,
        namespace: NamespaceSym,
        name: Sym,
        kind: TypeNameKind,
    ) -> TypeNameSym {
        let key = (namespace, name, kind);
        if let Some(&tn) = self.map.get(&key) {
            return tn;
        }
        let tn = TypeNameSym(self.rev.len() as u32);
        self.rev.push(key);
        self.map.insert(key, tn);
        tn
    }

    pub fn lookup(&self, tn: TypeNameSym) -> (NamespaceSym, Sym, TypeNameKind) {
        self.rev[tn.0 as usize]
    }

    pub fn name_of(&self, tn: TypeNameSym) -> Sym {
        self.lookup(tn).1
    }

    pub fn namespace_of(&self, tn: TypeNameSym) -> NamespaceSym {
        self.lookup(tn).0
    }

    pub fn kind_of(&self, tn: TypeNameSym) -> TypeNameKind {
        self.lookup(tn).2
    }

    /// `RBS::TypeName#with_prefix(namespace)` =
    /// `TypeName.new(namespace: namespace + self.namespace, name: self.name)`.
    pub fn with_prefix(&mut self, prefix: NamespaceSym, inner: TypeNameSym) -> TypeNameSym {
        let (inner_ns, inner_name, inner_kind) = self.lookup(inner);
        let combined = self.namespaces.join(prefix, inner_ns);
        self.intern(combined, inner_name, inner_kind)
    }

    /// `RBS::TypeName#to_namespace` = `namespace.append(name)`.
    pub fn to_namespace(&mut self, tn: TypeNameSym) -> NamespaceSym {
        let (ns, name, _) = self.lookup(tn);
        self.namespaces.append(ns, name)
    }

    pub fn to_string(&self, tn: TypeNameSym) -> String {
        let (ns, name, _) = self.lookup(tn);
        let (segs, absolute) = self.namespaces.lookup(ns);
        let mut out = String::new();
        if *absolute {
            out.push_str("::");
        }
        for seg in segs {
            out.push_str(self.symbols.lookup(*seg));
            out.push_str("::");
        }
        out.push_str(self.symbols.lookup(name));
        out
    }

    /// Borrow `self` as a read-only [`FrozenInterner`] view. Use this when handing
    /// the interner to the resolver or any other consumer that should
    /// not mutate it.
    pub fn frozen(&self) -> FrozenInterner<'_> {
        FrozenInterner { inner: self }
    }

    fn find(&self, namespace: NamespaceSym, name: Sym, kind: TypeNameKind) -> Option<TypeNameSym> {
        self.map.get(&(namespace, name, kind)).copied()
    }
}

// ---------- FrozenInterner views ----------
//
// Borrow newtypes that re-expose the read side of the three interners
// under the same operation names as their mutating counterparts
// (`intern`, `with_prefix`, `to_namespace`, `join`, `append`, …). Each
// "would-add-an-entry" call returns `Option`: the operation succeeds
// only if the entry has already been interned. A `None` from `FrozenInterner`
// therefore says "not in the table"; the resolver treats that as
// equivalent to "no declaration matches".

/// Read-only borrow of a [`TypeNameInterner`]. Construct via
/// [`TypeNameInterner::frozen`].
#[derive(Debug, Copy, Clone)]
pub struct FrozenInterner<'a> {
    inner: &'a TypeNameInterner,
}

impl<'a> FrozenInterner<'a> {
    pub fn new(interner: &'a TypeNameInterner) -> Self {
        Self { inner: interner }
    }

    pub fn namespaces(self) -> FrozenNamespaces<'a> {
        FrozenNamespaces(&self.inner.namespaces)
    }

    pub fn symbols(self) -> FrozenSymbols<'a> {
        FrozenSymbols(&self.inner.symbols)
    }

    pub fn intern(
        &self,
        namespace: NamespaceSym,
        name: Sym,
        kind: TypeNameKind,
    ) -> Option<TypeNameSym> {
        self.inner.find(namespace, name, kind)
    }

    pub fn with_prefix(&self, prefix: NamespaceSym, inner: TypeNameSym) -> Option<TypeNameSym> {
        let (inner_ns, inner_name, inner_kind) = self.inner.lookup(inner);
        let combined = self.namespaces().join(prefix, inner_ns)?;
        self.intern(combined, inner_name, inner_kind)
    }

    pub fn to_namespace(&self, tn: TypeNameSym) -> Option<NamespaceSym> {
        let (ns, name, _) = self.inner.lookup(tn);
        self.namespaces().append(ns, name)
    }

    pub fn lookup(&self, tn: TypeNameSym) -> (NamespaceSym, Sym, TypeNameKind) {
        self.inner.lookup(tn)
    }

    pub fn name_of(&self, tn: TypeNameSym) -> Sym {
        self.inner.name_of(tn)
    }

    pub fn namespace_of(&self, tn: TypeNameSym) -> NamespaceSym {
        self.inner.namespace_of(tn)
    }

    pub fn kind_of(&self, tn: TypeNameSym) -> TypeNameKind {
        self.inner.kind_of(tn)
    }

    pub fn to_string(&self, tn: TypeNameSym) -> String {
        self.inner.to_string(tn)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct FrozenNamespaces<'a>(&'a NamespaceInterner);

impl<'a> FrozenNamespaces<'a> {
    pub fn intern(&self, path: &[Sym], absolute: bool) -> Option<NamespaceSym> {
        self.0.find(path, absolute)
    }

    pub fn join(&self, lhs: NamespaceSym, rhs: NamespaceSym) -> Option<NamespaceSym> {
        self.0.join_lookup(lhs, rhs).ok()
    }

    pub fn append(&self, ns: NamespaceSym, seg: Sym) -> Option<NamespaceSym> {
        self.0.append_lookup(ns, seg).ok()
    }

    pub fn to_type_name(&self, ns: NamespaceSym) -> Option<(NamespaceSym, Sym)> {
        let (path, absolute) = self.0.lookup(ns);
        let last = *path.last()?;
        let mut parent = path.clone();
        parent.pop();
        let parent_ns = self.intern(&parent, *absolute)?;
        Some((parent_ns, last))
    }

    pub fn lookup(&self, ns: NamespaceSym) -> &'a (Vec<Sym>, bool) {
        self.0.lookup(ns)
    }

    pub fn empty_relative(&self) -> NamespaceSym {
        self.0.empty_relative()
    }

    pub fn root_absolute(&self) -> NamespaceSym {
        self.0.root_absolute()
    }

    pub fn is_empty(&self, ns: NamespaceSym) -> bool {
        self.0.is_empty(ns)
    }

    pub fn is_relative(&self, ns: NamespaceSym) -> bool {
        self.0.is_relative(ns)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct FrozenSymbols<'a>(&'a SymbolInterner);

impl<'a> FrozenSymbols<'a> {
    pub fn intern(&self, s: &str) -> Option<Sym> {
        self.0.find(s)
    }

    pub fn lookup(&self, sym: Sym) -> &'a str {
        self.0.lookup(sym)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_strings_dedups() {
        let mut s = SymbolInterner::new();
        let a = s.intern("foo");
        let b = s.intern("foo");
        let c = s.intern("bar");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(s.lookup(a), "foo");
    }

    #[test]
    fn type_name_with_prefix() {
        let mut tn = TypeNameInterner::new();
        let foo_sym = tn.symbols.intern("Foo");
        let bar_sym = tn.symbols.intern("Bar");
        let empty = tn.namespaces.empty_relative();
        let foo = tn.intern(empty, foo_sym, TypeNameKind::Class);
        let foo_ns = tn.to_namespace(foo);
        let bar_in_foo = tn.intern(foo_ns, bar_sym, TypeNameKind::Class);
        assert_eq!(tn.to_string(bar_in_foo), "Foo::Bar");

        let bar_in_foo_again = tn.intern(foo_ns, bar_sym, TypeNameKind::Class);
        assert_eq!(bar_in_foo, bar_in_foo_again);
    }

    #[test]
    fn kind_detect() {
        assert_eq!(TypeNameKind::detect("Foo"), TypeNameKind::Class);
        assert_eq!(TypeNameKind::detect("_Each"), TypeNameKind::Interface);
        assert_eq!(TypeNameKind::detect("foo_t"), TypeNameKind::TypeAlias);
    }

    #[test]
    fn namespace_parent_drops_last_segment_preserving_absolute() {
        let mut ni = NamespaceInterner::new();
        let foo = ni.intern(&[Sym(0)], true);
        let foo_bar = ni.append(foo, Sym(1));
        // ::Foo::Bar -> ::Foo
        assert_eq!(ni.parent(foo_bar), Some(foo));
        // ::Foo -> ::
        assert_eq!(ni.parent(foo), Some(ni.root_absolute()));
        // Relative form preserves absolute=false.
        let rel_foo = ni.intern(&[Sym(0)], false);
        assert_eq!(ni.parent(rel_foo), Some(ni.empty_relative()));
        // Empty namespaces have no parent.
        assert_eq!(ni.parent(ni.root_absolute()), None);
        assert_eq!(ni.parent(ni.empty_relative()), None);
    }

    #[test]
    fn namespace_to_type_name_splits_or_returns_none_for_empty() {
        let mut ni = NamespaceInterner::new();
        let foo = ni.intern(&[Sym(0)], true);
        let foo_bar = ni.append(foo, Sym(1));
        // ::Foo::Bar -> (::Foo, Bar)
        assert_eq!(ni.to_type_name(foo_bar), Some((foo, Sym(1))));
        // Empty namespaces cannot be split.
        assert_eq!(ni.to_type_name(ni.root_absolute()), None);
        assert_eq!(ni.to_type_name(ni.empty_relative()), None);
    }

    #[test]
    fn frozen_intern_misses_for_uninterned_typename() {
        let mut interner = TypeNameInterner::new();
        let foo = interner.symbols.intern("Foo");
        let root = interner.namespaces.root_absolute();
        // Not yet interned through TypeNameInterner::intern.
        assert_eq!(
            interner.frozen().intern(root, foo, TypeNameKind::Class),
            None
        );
        // After interning, frozen lookup hits.
        let tn = interner.intern(root, foo, TypeNameKind::Class);
        assert_eq!(
            interner.frozen().intern(root, foo, TypeNameKind::Class),
            Some(tn),
        );
    }

    #[test]
    fn frozen_join_returns_none_when_path_uninterned() {
        let mut interner = TypeNameInterner::new();
        let foo = interner.symbols.intern("Foo");
        let bar = interner.symbols.intern("Bar");
        let root = interner.namespaces.root_absolute();
        let foo_abs = interner.namespaces.intern(&[foo], true);
        let bar_rel = interner.namespaces.intern(&[bar], false);
        // join would produce [Foo, Bar] absolute, which has not been
        // interned yet.
        assert_eq!(interner.frozen().namespaces().join(foo_abs, bar_rel), None);
        // Mutating join interns it; frozen now succeeds.
        let combined = interner.namespaces.join(foo_abs, bar_rel);
        assert_eq!(
            interner.frozen().namespaces().join(foo_abs, bar_rel),
            Some(combined),
        );
        // Absolute rhs short-circuits regardless of intern state.
        let baz_abs = interner
            .namespaces
            .intern(&[interner.symbols.intern("Baz")], true);
        assert_eq!(
            interner.frozen().namespaces().join(root, baz_abs),
            Some(baz_abs),
        );
    }
}
