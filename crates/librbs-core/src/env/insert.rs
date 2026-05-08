use ruby_rbs::node::{Node, SignatureNode, TypeNameNode};

use crate::env::Environment;
use crate::env::entry::{
    ClassAliasEntry, ClassAliasLikeEntry, ClassEntry, ClassLikeEntry, ConstantEntry, Context,
    DeclRef, GlobalEntry, InterfaceEntry, ModuleAliasEntry, ModuleEntry, TypeAliasEntry,
};
use crate::error::{Error, Result};
use crate::interner::{NamespaceSym, Sym, TypeNameInterner, TypeNameKind, TypeNameSym};

/// Walk one parsed signature and register its declarations into `env`.
pub fn insert_rbs_source(
    env: &mut Environment,
    source_index: u32,
    signature: &SignatureNode<'_>,
) -> Result<()> {
    let mut counter: u32 = 0;
    // Mirrors `prefix: Namespace.root` in `RBS::Environment#resolve_signature`
    // (vendor/rbs/lib/rbs/environment.rb:515) — top-level decls are
    // anchored at the absolute root, so every entry's name is rendered
    // as `::Foo`, `::Foo::Bar`, etc. when stringified.
    let root = env.interner.namespaces.root_absolute();
    let context: Context = Vec::new();
    for decl in signature.declarations().iter() {
        insert_decl(env, source_index, &mut counter, &decl, &context, root)?;
    }
    Ok(())
}

fn assign_decl_index(counter: &mut u32, source_index: u32) -> DeclRef {
    let idx = *counter;
    *counter += 1;
    DeclRef {
        source_index,
        decl_index: idx,
    }
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

fn insert_decl(
    env: &mut Environment,
    source_index: u32,
    counter: &mut u32,
    decl: &Node<'_>,
    context: &Context,
    namespace: NamespaceSym,
) -> Result<()> {
    let decl_ref = assign_decl_index(counter, source_index);

    match decl {
        Node::Class(c) => {
            let inner = intern_type_name_node(&mut env.interner, &c.name());
            let name = env.interner.with_prefix(namespace, inner);
            check_constant_collision(env, name)?;
            let existing = env.class_decls.get(&name);
            match existing {
                Some(ClassLikeEntry::Module(_)) => {
                    return Err(Error::DuplicatedDeclaration {
                        name: env.interner.to_string(name),
                    });
                }
                Some(ClassLikeEntry::Class(_)) | None => {}
            }
            env.class_decls
                .entry(name)
                .or_insert_with(|| {
                    ClassLikeEntry::Class(ClassEntry {
                        name,
                        context_decls: Vec::new(),
                    })
                })
                .push(context.clone(), decl_ref);

            let inner_ns = env.interner.to_namespace(name);
            let mut inner_ctx = context.clone();
            inner_ctx.push(name);
            for member in c.members().iter() {
                if is_decl_node(&member) {
                    insert_decl(env, source_index, counter, &member, &inner_ctx, inner_ns)?;
                }
            }
        }
        Node::Module(m) => {
            let inner = intern_type_name_node(&mut env.interner, &m.name());
            let name = env.interner.with_prefix(namespace, inner);
            check_constant_collision(env, name)?;
            let existing = env.class_decls.get(&name);
            match existing {
                Some(ClassLikeEntry::Class(_)) => {
                    return Err(Error::DuplicatedDeclaration {
                        name: env.interner.to_string(name),
                    });
                }
                Some(ClassLikeEntry::Module(_)) | None => {}
            }
            env.class_decls
                .entry(name)
                .or_insert_with(|| {
                    ClassLikeEntry::Module(ModuleEntry {
                        name,
                        context_decls: Vec::new(),
                    })
                })
                .push(context.clone(), decl_ref);

            let inner_ns = env.interner.to_namespace(name);
            let mut inner_ctx = context.clone();
            inner_ctx.push(name);
            for member in m.members().iter() {
                if is_decl_node(&member) {
                    insert_decl(env, source_index, counter, &member, &inner_ctx, inner_ns)?;
                }
            }
        }
        Node::Interface(i) => {
            let inner = intern_type_name_node(&mut env.interner, &i.name());
            let name = env.interner.with_prefix(namespace, inner);
            if env.interface_decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }
            env.interface_decls.insert(
                name,
                InterfaceEntry {
                    name,
                    context: context.clone(),
                    decl: decl_ref,
                },
            );
        }
        Node::TypeAlias(a) => {
            let inner = intern_type_name_node(&mut env.interner, &a.name());
            let name = env.interner.with_prefix(namespace, inner);
            if env.type_alias_decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }
            env.type_alias_decls.insert(
                name,
                TypeAliasEntry {
                    name,
                    context: context.clone(),
                    decl: decl_ref,
                },
            );
        }
        Node::Constant(c) => {
            let inner = intern_type_name_node(&mut env.interner, &c.name());
            let name = env.interner.with_prefix(namespace, inner);
            check_constant_collision(env, name)?;
            if env.class_decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }
            env.constant_decls.insert(
                name,
                ConstantEntry {
                    name,
                    context: context.clone(),
                    decl: decl_ref,
                },
            );
        }
        Node::Global(g) => {
            let name_node = g.name();
            let name = env.interner.symbols.intern(name_node.as_str());
            if env.global_decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.symbols.lookup(name).to_string(),
                });
            }
            env.global_decls.insert(
                name,
                GlobalEntry {
                    name,
                    context: context.clone(),
                    decl: decl_ref,
                },
            );
        }
        Node::ClassAlias(ca) => {
            let inner = intern_type_name_node(&mut env.interner, &ca.new_name());
            let name = env.interner.with_prefix(namespace, inner);
            let old_name = intern_type_name_node(&mut env.interner, &ca.old_name());
            check_constant_collision(env, name)?;
            if env.class_decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }
            env.class_alias_decls.insert(
                name,
                ClassAliasLikeEntry::Class(ClassAliasEntry {
                    name,
                    old_name,
                    context: context.clone(),
                    decl: decl_ref,
                }),
            );
        }
        Node::ModuleAlias(ma) => {
            let inner = intern_type_name_node(&mut env.interner, &ma.new_name());
            let name = env.interner.with_prefix(namespace, inner);
            let old_name = intern_type_name_node(&mut env.interner, &ma.old_name());
            check_constant_collision(env, name)?;
            if env.class_decls.contains_key(&name) {
                return Err(Error::DuplicatedDeclaration {
                    name: env.interner.to_string(name),
                });
            }
            env.class_alias_decls.insert(
                name,
                ClassAliasLikeEntry::Module(ModuleAliasEntry {
                    name,
                    old_name,
                    context: context.clone(),
                    decl: decl_ref,
                }),
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
/// in the six `*_decls` hashes; `resolver::driver` recurses through the
/// same set when traversing class/module bodies; `canonical` emits one
/// fragment per such node. Keeping the three modules in lock-step
/// requires a single definition, which is here.
pub(crate) fn is_decl_node(n: &Node<'_>) -> bool {
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

fn check_constant_collision(env: &Environment, name: TypeNameSym) -> Result<()> {
    if env.constant_decls.contains_key(&name) || env.class_alias_decls.contains_key(&name) {
        return Err(Error::DuplicatedDeclaration {
            name: env.interner.to_string(name),
        });
    }
    Ok(())
}
