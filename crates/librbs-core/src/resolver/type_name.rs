//! Port of `RBS::Resolver::TypeNameResolver`.
//!
//! The Rust port follows the upstream Ruby control flow segment-for-segment
//! so that future divergences are easy to spot in review. The only
//! semantic differences:
//!
//! - Upstream's "context" is a `[outer, inner]` cons-cell list with
//!   sentinel `false` entries for non-class scopes. Our [`Context`] is a
//!   plain `Vec<TypeNameSym>` of class/module names from outer to inner;
//!   we never need the `false` sentinel because non-class scopes don't
//!   contribute namespaces in the first place. Unwinding the stack is
//!   `slice::split_last` instead of pattern-matching pairs.
//! - Upstream returns `false` to signal "cycle detected, stop here".
//!   We collapse `false` and `nil` into `Option::None`; any caller that
//!   wanted to distinguish them in upstream falls through to the same
//!   "no resolution" path anyway.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::env::{Context, DeclEntry, Environment};
use crate::interner::{FrozenInterner, Sym, TypeNameKind, TypeNameSym};

#[derive(Debug)]
pub struct TypeNameResolver {
    all_names: FxHashSet<TypeNameSym>,
    aliases: FxHashMap<TypeNameSym, (TypeNameSym, Context)>,
    cache: FxHashMap<(TypeNameSym, Context), Option<TypeNameSym>>,
}

impl TypeNameResolver {
    /// Mirror of `TypeNameResolver.build`: seed `all_names` from class,
    /// interface and type-alias decls, and `aliases` from class/module
    /// alias decls (`new_name → (old_name, context)`).
    pub fn build(env: &Environment) -> Self {
        let mut all_names: FxHashSet<TypeNameSym> = FxHashSet::default();
        let mut aliases: FxHashMap<TypeNameSym, (TypeNameSym, Context)> = FxHashMap::default();
        for (&name, entry) in &env.decls {
            match entry {
                DeclEntry::Class
                | DeclEntry::Module
                | DeclEntry::Interface
                | DeclEntry::TypeAlias => {
                    all_names.insert(name);
                }
                DeclEntry::ClassAlias(alias) => {
                    aliases.insert(name, (alias.old_name, alias.context.clone()));
                }
                DeclEntry::Constant => {}
            }
        }

        Self {
            all_names,
            aliases,
            cache: FxHashMap::default(),
        }
    }

    fn has_type_name(&self, full: TypeNameSym) -> bool {
        self.all_names.contains(&full)
    }

    fn aliased_name(&self, full: TypeNameSym) -> bool {
        self.aliases.contains_key(&full)
    }

    /// `RBS::Resolver::TypeNameResolver#resolve`. The result is cached on
    /// `(type_name, context)`; cache hits short-circuit the entire walk.
    ///
    /// Read-only against the interner: every candidate is looked up
    /// through [`FrozenInterner`], never interned. The insert phase is expected
    /// to have already interned every declaration (via
    /// [`TypeNameInterner::with_prefix`] in `env::insert::insert_decl`)
    /// and every reference reachable from a signature, so a `FrozenInterner`
    /// miss is sufficient evidence that the candidate is not a
    /// declaration and the walk can fall through.
    pub fn resolve(
        &mut self,
        type_name: TypeNameSym,
        context: &Context,
        interner: FrozenInterner<'_>,
    ) -> Option<TypeNameSym> {
        let (ns, name, kind) = interner.lookup(type_name);
        let absolute = !interner.namespaces().is_relative(ns);

        if absolute && self.has_type_name(type_name) {
            return Some(type_name);
        }

        let key = (type_name, context.clone());
        if let Some(&cached) = self.cache.get(&key) {
            return cached;
        }

        let result = if matches!(kind, TypeNameKind::Class) {
            let mut visited: FxHashSet<TypeNameSym> = FxHashSet::default();
            self.resolve_namespace0(type_name, context, &mut visited, interner)
        } else if interner.namespaces().is_empty(ns) {
            self.resolve_type_name(name, context, interner)
        } else {
            // namespace.to_type_name → (parent, last). For an interface or
            // type-alias name like `Foo::Bar::_Each`, we resolve the
            // namespace `Foo::Bar` first, then re-attach `_Each`.
            //
            // If the parent path or the namespace-as-class candidate has
            // never been interned, no declaration uses that namespace as
            // a class scope, so resolution must fail — return `None`.
            (|| {
                let (parent_ns, last_seg) = interner.namespaces().to_type_name(ns)?;
                let ns_tn = interner.intern(parent_ns, last_seg, TypeNameKind::Class)?;
                let mut visited: FxHashSet<TypeNameSym> = FxHashSet::default();
                let resolved_ns_tn =
                    self.resolve_namespace0(ns_tn, context, &mut visited, interner)?;
                let resolved_ns = interner.to_namespace(resolved_ns_tn)?;
                let full = interner.intern(resolved_ns, name, kind)?;
                if self.has_type_name(full) {
                    Some(full)
                } else {
                    None
                }
            })()
        };

        self.cache.insert(key, result);
        result
    }

    /// `RBS::Resolver::TypeNameResolver#resolve_type_name`. Walks the
    /// context from innermost to outermost looking for a class scope that
    /// contains `name`; falls back to the absolute root.
    fn resolve_type_name(
        &self,
        name: Sym,
        context: &[TypeNameSym],
        interner: FrozenInterner<'_>,
    ) -> Option<TypeNameSym> {
        let kind = {
            let s = interner.symbols().lookup(name);
            TypeNameKind::detect(s)
        };
        if let Some((&inner, outer)) = context.split_last() {
            // `to_namespace` returning `None` means `inner` has no
            // children declared, so it can't host `name`; fall through
            // to the outer scope. Likewise, an un-interned candidate
            // tuple means no such declaration exists.
            if let Some(inner_ns) = interner.to_namespace(inner)
                && let Some(candidate) = interner.intern(inner_ns, name, kind)
                && self.has_type_name(candidate)
            {
                return Some(candidate);
            }
            self.resolve_type_name(name, outer, interner)
        } else {
            let root = interner.namespaces().root_absolute();
            let candidate = interner.intern(root, name, kind)?;
            if self.has_type_name(candidate) {
                Some(candidate)
            } else {
                None
            }
        }
    }

    /// `RBS::Resolver::TypeNameResolver#resolve_head_namespace`. Like
    /// `resolve_type_name`, but at each level a class alias entry is
    /// also accepted as a hit.
    fn resolve_head_namespace(
        &self,
        head: Sym,
        context: &[TypeNameSym],
        interner: FrozenInterner<'_>,
    ) -> Option<TypeNameSym> {
        if let Some((&inner, outer)) = context.split_last() {
            if let Some(inner_ns) = interner.to_namespace(inner)
                && let Some(candidate) = interner.intern(inner_ns, head, TypeNameKind::Class)
                && (self.has_type_name(candidate) || self.aliased_name(candidate))
            {
                return Some(candidate);
            }
            self.resolve_head_namespace(head, outer, interner)
        } else {
            let root = interner.namespaces().root_absolute();
            let candidate = interner.intern(root, head, TypeNameKind::Class)?;
            if self.has_type_name(candidate) || self.aliased_name(candidate) {
                Some(candidate)
            } else {
                None
            }
        }
    }

    /// `RBS::Resolver::TypeNameResolver#normalize_namespace`. Cycle-detects
    /// via the shared `visited` set and recurses through `resolve_namespace0`.
    fn normalize_namespace(
        &mut self,
        type_name: TypeNameSym,
        rhs: TypeNameSym,
        context: &Context,
        visited: &mut FxHashSet<TypeNameSym>,
        interner: FrozenInterner<'_>,
    ) -> Option<TypeNameSym> {
        if !visited.insert(type_name) {
            return None;
        }
        let result = self.resolve_namespace0(rhs, context, visited, interner);
        visited.remove(&type_name);
        result
    }

    /// `RBS::Resolver::TypeNameResolver#resolve_namespace0`. The core
    /// segment-by-segment walk: resolve the head, then traverse the tail,
    /// applying class-alias normalization at each step.
    fn resolve_namespace0(
        &mut self,
        type_name: TypeNameSym,
        context: &Context,
        visited: &mut FxHashSet<TypeNameSym>,
        interner: FrozenInterner<'_>,
    ) -> Option<TypeNameSym> {
        let (ns, name, _kind) = interner.lookup(type_name);
        let (path, absolute) = interner.namespaces().lookup(ns).clone();

        let mut segments: Vec<Sym> = path;
        segments.push(name);
        let head_sym = segments[0];

        let head_tn: Option<TypeNameSym> = if absolute {
            let root_name = interner.intern(
                interner.namespaces().root_absolute(),
                head_sym,
                TypeNameKind::Class,
            )?;
            if self.has_type_name(root_name) || self.aliased_name(root_name) {
                Some(root_name)
            } else {
                None
            }
        } else {
            self.resolve_head_namespace(head_sym, context, interner)
        };

        let mut acc = head_tn?;

        // Apply alias normalization at the head.
        if let Some((rhs, ctx)) = self.aliases.get(&acc).cloned() {
            acc = self.normalize_namespace(acc, rhs, &ctx, visited, interner)?;
        }

        // Walk the tail, resolving each segment against `acc.to_namespace`.
        // `to_namespace` / `intern` returning `None` means `acc` has no
        // child declaration matching `seg`, so resolution must fail —
        // there is no aliased fallback to consider when the candidate
        // itself was never interned.
        for &seg in &segments[1..] {
            let acc_ns = interner.to_namespace(acc)?;
            let kind = {
                let s = interner.symbols().lookup(seg);
                TypeNameKind::detect(s)
            };
            let candidate = interner.intern(acc_ns, seg, kind)?;
            if self.has_type_name(candidate) {
                acc = candidate;
            } else if let Some((rhs, ctx)) = self.aliases.get(&candidate).cloned() {
                acc = self.normalize_namespace(candidate, rhs, &ctx, visited, interner)?;
            } else {
                return None;
            }
        }

        Some(acc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{ClassAliasEntry, Environment};
    use crate::interner::{TypeNameInterner, TypeNameKind};

    fn intern_class(
        interner: &mut TypeNameInterner,
        absolute: bool,
        path: &[&str],
        name: &str,
    ) -> TypeNameSym {
        let segs: Vec<Sym> = path.iter().map(|s| interner.symbols.intern(s)).collect();
        let ns = interner.namespaces.intern(&segs, absolute);
        let name_sym = interner.symbols.intern(name);
        interner.intern(ns, name_sym, TypeNameKind::Class)
    }

    fn intern_interface(
        interner: &mut TypeNameInterner,
        absolute: bool,
        path: &[&str],
        name: &str,
    ) -> TypeNameSym {
        let segs: Vec<Sym> = path.iter().map(|s| interner.symbols.intern(s)).collect();
        let ns = interner.namespaces.intern(&segs, absolute);
        let name_sym = interner.symbols.intern(name);
        interner.intern(ns, name_sym, TypeNameKind::Interface)
    }

    fn add_class(env: &mut Environment, name: TypeNameSym) {
        env.decls.insert(name, DeclEntry::Class);
    }

    fn add_interface(env: &mut Environment, name: TypeNameSym) {
        env.decls.insert(name, DeclEntry::Interface);
    }

    fn add_class_alias(
        env: &mut Environment,
        new_name: TypeNameSym,
        old_name: TypeNameSym,
        context: Context,
    ) {
        env.decls.insert(
            new_name,
            DeclEntry::ClassAlias(Box::new(ClassAliasEntry { old_name, context })),
        );
    }

    #[test]
    fn resolve_absolute_known_returns_input() {
        let mut env = Environment::new();
        let foo = intern_class(&mut env.interner, true, &[], "Foo");
        add_class(&mut env, foo);

        let mut resolver = TypeNameResolver::build(&env);
        let resolved = resolver.resolve(foo, &Vec::new(), env.interner.frozen());
        assert_eq!(resolved, Some(foo));
    }

    #[test]
    fn resolve_unqualified_class_in_nested_context_walks_outer() {
        // Setup: ::Foo and ::Foo::Bar exist. From inside Foo::Bar, an
        // unqualified `Foo` should resolve to ::Foo by walking outer.
        let mut env = Environment::new();
        let foo = intern_class(&mut env.interner, true, &[], "Foo");
        let foo_bar = intern_class(&mut env.interner, true, &["Foo"], "Bar");
        add_class(&mut env, foo);
        add_class(&mut env, foo_bar);

        let context = vec![foo, foo_bar];
        let unq_foo = intern_class(&mut env.interner, false, &[], "Foo");
        let mut resolver = TypeNameResolver::build(&env);
        let resolved = resolver.resolve(unq_foo, &context, env.interner.frozen());
        assert_eq!(resolved, Some(foo));
    }

    #[test]
    fn resolve_unqualified_interface_in_nested_context() {
        // ::Foo::_Each interface; from inside Foo, `_Each` resolves there.
        let mut env = Environment::new();
        let foo = intern_class(&mut env.interner, true, &[], "Foo");
        let foo_each = intern_interface(&mut env.interner, true, &["Foo"], "_Each");
        add_class(&mut env, foo);
        add_interface(&mut env, foo_each);

        let context = vec![foo];
        let unq_each = intern_interface(&mut env.interner, false, &[], "_Each");
        let mut resolver = TypeNameResolver::build(&env);
        let resolved = resolver.resolve(unq_each, &context, env.interner.frozen());
        assert_eq!(resolved, Some(foo_each));
    }

    #[test]
    fn resolve_through_class_alias() {
        // class Real ; class Alias = Real (top-level). Inside no nesting,
        // `Alias::Inner` should normalize through Alias -> Real, then
        // attach `Inner` to find ::Real::Inner.
        let mut env = Environment::new();
        let real = intern_class(&mut env.interner, true, &[], "Real");
        let real_inner = intern_class(&mut env.interner, true, &["Real"], "Inner");
        let aliased = intern_class(&mut env.interner, true, &[], "AliasName");
        add_class(&mut env, real);
        add_class(&mut env, real_inner);
        add_class_alias(&mut env, aliased, real, Vec::new());

        let probe = intern_class(&mut env.interner, false, &["AliasName"], "Inner");
        let mut resolver = TypeNameResolver::build(&env);
        let resolved = resolver.resolve(probe, &Vec::new(), env.interner.frozen());
        assert_eq!(resolved, Some(real_inner));
    }

    #[test]
    fn alias_cycle_returns_none_cleanly() {
        // class A = B ; class B = A. Resolving either should not loop.
        let mut env = Environment::new();
        let a = intern_class(&mut env.interner, true, &[], "A");
        let b = intern_class(&mut env.interner, true, &[], "B");
        add_class_alias(&mut env, a, b, Vec::new());
        add_class_alias(&mut env, b, a, Vec::new());

        let probe = intern_class(&mut env.interner, false, &[], "A");
        let mut resolver = TypeNameResolver::build(&env);
        let resolved = resolver.resolve(probe, &Vec::new(), env.interner.frozen());
        assert_eq!(resolved, None);
    }

    #[test]
    fn resolve_unknown_name_returns_none() {
        let mut env = Environment::new();
        let foo = intern_class(&mut env.interner, true, &[], "Foo");
        add_class(&mut env, foo);

        let probe = intern_class(&mut env.interner, false, &[], "Bar");
        let mut resolver = TypeNameResolver::build(&env);
        let resolved = resolver.resolve(probe, &Vec::new(), env.interner.frozen());
        assert_eq!(resolved, None);
    }
}
