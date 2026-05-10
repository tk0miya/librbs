use ruby_rbs::node::{
    BlockTypeNode, ClassNode, FunctionTypeNode, InterfaceNode, MethodTypeNode, ModuleNode, Node,
    NodeList, ProcTypeNode, RecordTypeNode, SignatureNode, TypeNameNode,
};

use crate::env::{ClassAliasEntry, Context, DeclEntry, Environment};
use crate::error::{Error, Result};
use crate::interner::{
    FrozenInterner, NamespaceSym, Sym, TypeNameInterner, TypeNameKind, TypeNameSym,
};

/// Walk one parsed signature and register its declarations into `env`.
pub fn insert_rbs_source(env: &mut Environment, signature: &SignatureNode<'_>) -> Result<()> {
    // Mirrors `prefix: Namespace.root` in `RBS::Environment#resolve_signature`
    // (vendor/rbs/lib/rbs/environment.rb:515) — top-level decls are
    // anchored at the absolute root, so every entry's name is rendered
    // as `::Foo`, `::Foo::Bar`, etc. when stringified.
    let root = env.interner.namespaces.root_absolute();
    let context: Context = Vec::new();
    for decl in signature.declarations().iter() {
        insert_decl(env, &decl, &context, root)?;
    }
    Ok(())
}

/// Convert a parsed `TypeNameNode` into a [`TypeNameSym`] by interning
/// every segment and the leaf name through `interner`. Both the M2
/// insert pass (entry registration) and the M3b resolver driver
/// (recording type-name occurrences) need to perform exactly this
/// translation, so the shared definition lives here.
pub(crate) fn intern_type_name_node(
    interner: &mut TypeNameInterner,
    node: &TypeNameNode<'_>,
) -> TypeNameSym {
    let ns_node = node.namespace();
    let absolute = ns_node.absolute();
    let mut path: Vec<Sym> = Vec::new();
    for seg in ns_node.path().iter() {
        if let Node::Symbol(sym) = seg {
            path.push(interner.symbols.intern(sym.as_str()));
        }
    }
    let name_node = node.name();
    let name = interner.symbols.intern(name_node.as_str());
    let ns = interner.namespaces.intern(&path, absolute);
    let kind = TypeNameKind::detect(name_node.as_str());
    interner.intern(ns, name, kind)
}

/// Read-only counterpart of [`intern_type_name_node`]. Returns
/// `Some(sym)` only if every segment, the leaf name, the namespace
/// path and the resulting `(ns, name, kind)` tuple have already been
/// interned. After [`insert_rbs_source`] has run on a signature, every
/// `TypeNameNode` reachable from one of its declarations is guaranteed
/// to be findable via this function (see the reference-interning
/// walkers below); callers in the resolver driver can therefore avoid
/// taking `&mut TypeNameInterner` entirely.
pub fn find_type_name_node(
    interner: FrozenInterner<'_>,
    node: &TypeNameNode<'_>,
) -> Option<TypeNameSym> {
    let ns_node = node.namespace();
    let absolute = ns_node.absolute();
    let mut path: Vec<Sym> = Vec::new();
    for seg in ns_node.path().iter() {
        if let Node::Symbol(sym) = seg {
            path.push(interner.symbols().intern(sym.as_str())?);
        }
    }
    let name_node = node.name();
    let name = interner.symbols().intern(name_node.as_str())?;
    let ns = interner.namespaces().intern(&path, absolute)?;
    let kind = TypeNameKind::detect(name_node.as_str());
    interner.intern(ns, name, kind)
}

// ----- reference-interning walkers -----
//
// These mirror the per-decl AST traversal in `resolver::driver` but only
// intern each type-name reference; resolution itself is left to the
// resolve phase. By the time `insert_rbs_source` returns, every
// `TypeNameNode` reachable from a declaration body — super-classes,
// self-types, mixins, method types, type aliases' bodies, etc. — has
// been added to `env.interner`, which lets the resolver run later
// against an immutable interner. `# use` directives are intentionally
// skipped: their wildcard form is only meaningful after every other
// decl is known, so they remain the resolver's responsibility.

fn intern_class_refs(interner: &mut TypeNameInterner, c: &ClassNode<'_>) {
    intern_type_params(interner, &c.type_params());
    if let Some(sc) = c.super_class() {
        let _ = intern_type_name_node(interner, &sc.name());
        for arg in sc.args().iter() {
            intern_type(interner, &arg);
        }
    }
}

fn intern_module_refs(interner: &mut TypeNameInterner, m: &ModuleNode<'_>) {
    intern_type_params(interner, &m.type_params());
    for st in m.self_types().iter() {
        if let Node::ModuleSelf(ms) = &st {
            let _ = intern_type_name_node(interner, &ms.name());
            for arg in ms.args().iter() {
                intern_type(interner, &arg);
            }
        }
    }
}

fn intern_interface_refs(interner: &mut TypeNameInterner, i: &InterfaceNode<'_>) {
    intern_type_params(interner, &i.type_params());
    for member in i.members().iter() {
        intern_member(interner, &member);
    }
}

fn intern_member(interner: &mut TypeNameInterner, member: &Node<'_>) {
    match member {
        Node::MethodDefinition(m) => {
            for overload in m.overloads().iter() {
                if let Node::MethodDefinitionOverload(o) = &overload
                    && let Node::MethodType(mt) = o.method_type()
                {
                    intern_method_type(interner, &mt);
                }
            }
        }
        Node::AttrAccessor(a) => intern_type(interner, &a.type_()),
        Node::AttrReader(a) => intern_type(interner, &a.type_()),
        Node::AttrWriter(a) => intern_type(interner, &a.type_()),
        Node::InstanceVariable(v) => intern_type(interner, &v.type_()),
        Node::ClassInstanceVariable(v) => intern_type(interner, &v.type_()),
        Node::ClassVariable(v) => intern_type(interner, &v.type_()),
        Node::Include(m) => intern_mixin(interner, &m.name(), m.args().iter()),
        Node::Extend(m) => intern_mixin(interner, &m.name(), m.args().iter()),
        Node::Prepend(m) => intern_mixin(interner, &m.name(), m.args().iter()),
        // Public, Private, Alias carry no type-name occurrences — same
        // fall-through as `walk_member` in resolver::driver.
        _ => {}
    }
}

fn intern_mixin<'a>(
    interner: &mut TypeNameInterner,
    name: &TypeNameNode<'a>,
    args: impl Iterator<Item = Node<'a>>,
) {
    let _ = intern_type_name_node(interner, name);
    for arg in args {
        intern_type(interner, &arg);
    }
}

fn intern_method_type(interner: &mut TypeNameInterner, mt: &MethodTypeNode<'_>) {
    intern_type_params(interner, &mt.type_params());
    intern_type(interner, &mt.type_());
    if let Some(block) = mt.block() {
        intern_block(interner, &block);
    }
}

fn intern_block(interner: &mut TypeNameInterner, b: &BlockTypeNode<'_>) {
    intern_type(interner, &b.type_());
    if let Some(self_t) = b.self_type() {
        intern_type(interner, &self_t);
    }
}

fn intern_type_params(interner: &mut TypeNameInterner, params: &NodeList<'_>) {
    for p in params.iter() {
        if let Node::TypeParam(tp) = &p {
            if let Some(ub) = tp.upper_bound() {
                intern_type(interner, &ub);
            }
            if let Some(lb) = tp.lower_bound() {
                intern_type(interner, &lb);
            }
            if let Some(dt) = tp.default_type() {
                intern_type(interner, &dt);
            }
        }
    }
}

fn intern_type(interner: &mut TypeNameInterner, ty: &Node<'_>) {
    match ty {
        Node::ClassInstanceType(t) => {
            let _ = intern_type_name_node(interner, &t.name());
            for arg in t.args().iter() {
                intern_type(interner, &arg);
            }
        }
        Node::InterfaceType(t) => {
            let _ = intern_type_name_node(interner, &t.name());
            for arg in t.args().iter() {
                intern_type(interner, &arg);
            }
        }
        Node::AliasType(t) => {
            let _ = intern_type_name_node(interner, &t.name());
            for arg in t.args().iter() {
                intern_type(interner, &arg);
            }
        }
        Node::ClassSingletonType(t) => {
            let _ = intern_type_name_node(interner, &t.name());
        }
        Node::TupleType(t) => {
            for el in t.types().iter() {
                intern_type(interner, &el);
            }
        }
        Node::UnionType(t) => {
            for el in t.types().iter() {
                intern_type(interner, &el);
            }
        }
        Node::IntersectionType(t) => {
            for el in t.types().iter() {
                intern_type(interner, &el);
            }
        }
        Node::RecordType(t) => intern_record_type(interner, t),
        Node::OptionalType(t) => intern_type(interner, &t.type_()),
        Node::ProcType(t) => intern_proc_type(interner, t),
        Node::FunctionType(t) => intern_function_type(interner, t),
        Node::UntypedFunctionType(t) => intern_type(interner, &t.return_type()),
        Node::BlockType(b) => intern_block(interner, b),
        Node::RecordFieldType(f) => intern_type(interner, &f.type_()),
        // Literal / Variable / Bool / Void / Any / Nil / Top / Bottom /
        // Self / Instance / Class — none carry resolvable type names.
        _ => {}
    }
}

fn intern_record_type(interner: &mut TypeNameInterner, t: &RecordTypeNode<'_>) {
    for (_, value) in t.all_fields().iter() {
        if let Node::RecordFieldType(f) = &value {
            intern_type(interner, &f.type_());
        }
    }
}

fn intern_proc_type(interner: &mut TypeNameInterner, t: &ProcTypeNode<'_>) {
    intern_type(interner, &t.type_());
    if let Some(b) = t.block() {
        intern_block(interner, &b);
    }
    if let Some(st) = t.self_type() {
        intern_type(interner, &st);
    }
}

fn intern_function_type(interner: &mut TypeNameInterner, t: &FunctionTypeNode<'_>) {
    for p in t.required_positionals().iter() {
        intern_function_param(interner, &p);
    }
    for p in t.optional_positionals().iter() {
        intern_function_param(interner, &p);
    }
    if let Some(rest) = t.rest_positionals() {
        intern_function_param(interner, &rest);
    }
    for p in t.trailing_positionals().iter() {
        intern_function_param(interner, &p);
    }
    for (_, value) in t.required_keywords().iter() {
        intern_function_param(interner, &value);
    }
    for (_, value) in t.optional_keywords().iter() {
        intern_function_param(interner, &value);
    }
    if let Some(rest) = t.rest_keywords() {
        intern_function_param(interner, &rest);
    }
    intern_type(interner, &t.return_type());
}

fn intern_function_param(interner: &mut TypeNameInterner, p: &Node<'_>) {
    if let Node::FunctionParam(fp) = p {
        intern_type(interner, &fp.type_());
    } else {
        intern_type(interner, p);
    }
}

fn insert_decl(
    env: &mut Environment,
    decl: &Node<'_>,
    context: &Context,
    namespace: NamespaceSym,
) -> Result<()> {
    match decl {
        Node::Class(c) => {
            let inner = intern_type_name_node(&mut env.interner, &c.name());
            let name = env.interner.with_prefix(namespace, inner);
            match env.decls.get(&name) {
                Some(DeclEntry::Class) | None => {}
                Some(_) => {
                    return Err(Error::DuplicatedDeclaration {
                        name: env.interner.to_string(name),
                    });
                }
            }
            env.decls.entry(name).or_insert(DeclEntry::Class);

            intern_class_refs(&mut env.interner, c);

            let inner_ns = env.interner.to_namespace(name);
            let mut inner_ctx = context.clone();
            inner_ctx.push(name);
            for member in c.members().iter() {
                if is_decl_node(&member) {
                    insert_decl(env, &member, &inner_ctx, inner_ns)?;
                } else {
                    intern_member(&mut env.interner, &member);
                }
            }
        }
        Node::Module(m) => {
            let inner = intern_type_name_node(&mut env.interner, &m.name());
            let name = env.interner.with_prefix(namespace, inner);
            match env.decls.get(&name) {
                Some(DeclEntry::Module) | None => {}
                Some(_) => {
                    return Err(Error::DuplicatedDeclaration {
                        name: env.interner.to_string(name),
                    });
                }
            }
            env.decls.entry(name).or_insert(DeclEntry::Module);

            intern_module_refs(&mut env.interner, m);

            let inner_ns = env.interner.to_namespace(name);
            let mut inner_ctx = context.clone();
            inner_ctx.push(name);
            for member in m.members().iter() {
                if is_decl_node(&member) {
                    insert_decl(env, &member, &inner_ctx, inner_ns)?;
                } else {
                    intern_member(&mut env.interner, &member);
                }
            }
        }
        Node::Interface(i) => {
            let inner = intern_type_name_node(&mut env.interner, &i.name());
            let name = env.interner.with_prefix(namespace, inner);
            // Interface names live in their own `TypeNameKind`-segregated
            // sym space, so any prior entry under `name` is necessarily
            // another interface — i.e. a duplicate.
            if env.decls.insert(name, DeclEntry::Interface).is_some() {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }

            intern_interface_refs(&mut env.interner, i);
        }
        Node::TypeAlias(a) => {
            let inner = intern_type_name_node(&mut env.interner, &a.name());
            let name = env.interner.with_prefix(namespace, inner);
            if env.decls.insert(name, DeclEntry::TypeAlias).is_some() {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }

            intern_type_params(&mut env.interner, &a.type_params());
            intern_type(&mut env.interner, &a.type_());
        }
        Node::Constant(c) => {
            let inner = intern_type_name_node(&mut env.interner, &c.name());
            let name = env.interner.with_prefix(namespace, inner);
            if env.decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }
            env.decls.insert(name, DeclEntry::Constant);

            intern_type(&mut env.interner, &c.type_());
        }
        Node::Global(g) => {
            let name_node = g.name();
            let name = env.interner.symbols.intern(name_node.as_str());
            if !env.global_decls.insert(name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.symbols.lookup(name).to_string(),
                });
            }

            intern_type(&mut env.interner, &g.type_());
        }
        Node::ClassAlias(ca) => {
            let inner = intern_type_name_node(&mut env.interner, &ca.new_name());
            let name = env.interner.with_prefix(namespace, inner);
            let old_name = intern_type_name_node(&mut env.interner, &ca.old_name());
            if env.decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }
            env.decls.insert(
                name,
                DeclEntry::ClassAlias(Box::new(ClassAliasEntry {
                    old_name,
                    context: context.clone(),
                })),
            );
        }
        Node::ModuleAlias(ma) => {
            let inner = intern_type_name_node(&mut env.interner, &ma.new_name());
            let name = env.interner.with_prefix(namespace, inner);
            let old_name = intern_type_name_node(&mut env.interner, &ma.old_name());
            if env.decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }
            env.decls.insert(
                name,
                DeclEntry::ClassAlias(Box::new(ClassAliasEntry {
                    old_name,
                    context: context.clone(),
                })),
            );
        }
        _ => {
            // Non-declaration nodes (e.g. members in a stray context) are
            // ignored by `insert_rbs_decl` in upstream RBS too.
        }
    }
    Ok(())
}

/// Predicate over [`Node`] variants that env entries are built from.
///
/// The eight kinds enumerated below are the canonical "what counts as a
/// top-level declaration" list. `env::insert` registers them as entries
/// in `Environment::decls` (or `global_decls` for `Global`);
/// `resolver::driver` recurses through the same set when traversing
/// class/module bodies; `canonical` emits one fragment per such node;
/// M3h's materializer (in the `ext/librbs` crate) likewise uses it to
/// dispatch class/module member walks between nested-decl recursion
/// and member materialization. Keeping these in lock-step requires a
/// single definition, which is here.
pub fn is_decl_node(n: &Node<'_>) -> bool {
    matches!(
        n,
        Node::Class(_)
            | Node::Module(_)
            | Node::Interface(_)
            | Node::TypeAlias(_)
            | Node::Constant(_)
            | Node::Global(_)
            | Node::ClassAlias(_)
            | Node::ModuleAlias(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ManagedParser;

    fn parse_and_insert(src: &str) -> Environment {
        let parser = ManagedParser::parse(src.to_string()).unwrap();
        let mut env = Environment::new();
        insert_rbs_source(&mut env, parser.signature()).unwrap();
        env
    }

    /// After insert, asserts that `name` is already present in the symbol
    /// interner. We search the symbol table directly so the check stays
    /// `&Environment` and doesn't risk masking a missing intern by
    /// allocating fresh.
    fn assert_symbol_interned(env: &Environment, name: &str) {
        let total = env.interner.symbols.len();
        let found = (0..total).any(|i| env.interner.symbols.lookup(Sym(i as u32)) == name);
        assert!(found, "expected symbol {name:?} to be interned");
    }

    #[test]
    fn class_super_class_reference_is_interned() {
        let env = parse_and_insert("class Bar < Foo end\n");
        assert_symbol_interned(&env, "Foo");
    }

    #[test]
    fn module_self_type_reference_is_interned() {
        let env = parse_and_insert("module M : _Each end\n");
        assert_symbol_interned(&env, "_Each");
    }

    #[test]
    fn mixin_references_are_interned() {
        let env =
            parse_and_insert("class Foo\n  include Mixed\n  extend Hooked\n  prepend Front\nend\n");
        assert_symbol_interned(&env, "Mixed");
        assert_symbol_interned(&env, "Hooked");
        assert_symbol_interned(&env, "Front");
    }

    #[test]
    fn method_type_references_are_interned() {
        let env = parse_and_insert(
            "class Foo\n  def bar: (Arg) -> Ret\n  def baz: () { (BlockArg) -> BlockRet } -> Yielder\nend\n",
        );
        for n in ["Arg", "Ret", "BlockArg", "BlockRet", "Yielder"] {
            assert_symbol_interned(&env, n);
        }
    }

    #[test]
    fn attr_and_ivar_type_references_are_interned() {
        let env = parse_and_insert(
            "class Foo\n  attr_reader name: NameType\n  @count: CountType\n  @@total: TotalType\n  self.@cache: CacheType\nend\n",
        );
        for n in ["NameType", "CountType", "TotalType", "CacheType"] {
            assert_symbol_interned(&env, n);
        }
    }

    #[test]
    fn type_alias_body_references_are_interned() {
        let env = parse_and_insert("type x = Integer | String\n");
        assert_symbol_interned(&env, "Integer");
        assert_symbol_interned(&env, "String");
    }

    #[test]
    fn constant_type_reference_is_interned() {
        let env = parse_and_insert("Pi: Numeric\n");
        assert_symbol_interned(&env, "Numeric");
    }

    #[test]
    fn global_type_reference_is_interned() {
        let env = parse_and_insert("$logger: Logger\n");
        assert_symbol_interned(&env, "Logger");
    }

    #[test]
    fn interface_method_type_references_are_interned() {
        let env = parse_and_insert("interface _Each\n  def each: () -> EachReturn\nend\n");
        assert_symbol_interned(&env, "EachReturn");
    }

    #[test]
    fn nested_decls_intern_outer_and_inner_references() {
        // Inner class references must be interned through the recursive
        // member walk.
        let env =
            parse_and_insert("class Outer < OuterParent\n  class Inner < InnerParent end\nend\n");
        assert_symbol_interned(&env, "OuterParent");
        assert_symbol_interned(&env, "InnerParent");
    }

    #[test]
    fn type_param_bounds_are_interned() {
        let env =
            parse_and_insert("class Foo[T < UpperBoundType] end\ntype y[U < AliasUpper] = U\n");
        assert_symbol_interned(&env, "UpperBoundType");
        assert_symbol_interned(&env, "AliasUpper");
    }

    #[test]
    fn record_and_tuple_types_are_interned() {
        let env =
            parse_and_insert("type r = { name: RecName, age: RecAge }\ntype t = [TupA, TupB]\n");
        for n in ["RecName", "RecAge", "TupA", "TupB"] {
            assert_symbol_interned(&env, n);
        }
    }

    #[test]
    fn relative_reference_typename_is_interned_alongside_absolute_lhs() {
        // `class Foo end` defines ::Foo; `class Bar < Foo` references the
        // *relative* `Foo`. After insert, both the absolute LHS and the
        // relative reference must be in the typename interner — re-
        // interning either does not grow the symbol table or allocate a
        // new `Sym`.
        let mut env = parse_and_insert("class Foo end\nclass Bar < Foo end\n");
        let sym_len_before = env.interner.symbols.len();
        let foo_sym = env.interner.symbols.intern("Foo");
        assert_eq!(
            env.interner.symbols.len(),
            sym_len_before,
            "Foo segment should already be interned"
        );

        let abs_root = env.interner.namespaces.root_absolute();
        let rel_empty = env.interner.namespaces.empty_relative();
        let abs_foo = env.interner.intern(abs_root, foo_sym, TypeNameKind::Class);
        let rel_foo = env.interner.intern(rel_empty, foo_sym, TypeNameKind::Class);
        assert_ne!(abs_foo, rel_foo, "namespace makes them distinct");
    }
}
