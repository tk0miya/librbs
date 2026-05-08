use ruby_rbs::node::{Node, SignatureNode, TypeNameNode};

use crate::env::Environment;
use crate::env::entry::{
    ClassAliasEntry, ClassAliasLikeEntry, ClassEntry, ClassLikeEntry, ConstantEntry, Context,
    DeclRef, GlobalEntry, InterfaceEntry, ModuleAliasEntry, ModuleEntry, TypeAliasEntry,
};
use crate::error::{Error, Result};
use crate::interner::{NamespaceSym, Sym, TypeNameKind, TypeNameSym};

/// Walk one parsed signature and register its declarations into `env`.
pub fn insert_rbs_source(
    env: &mut Environment,
    source_index: u32,
    signature: &SignatureNode<'_>,
) -> Result<()> {
    let mut counter: u32 = 0;
    let empty_ns = env.interner.namespaces.empty_relative();
    let context: Context = Vec::new();
    for decl in signature.declarations().iter() {
        insert_decl(env, source_index, &mut counter, &decl, &context, empty_ns)?;
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

fn intern_type_name(env: &mut Environment, node: &TypeNameNode<'_>) -> TypeNameSym {
    let ns_node = node.namespace();
    let absolute = ns_node.absolute();
    let mut path: Vec<Sym> = Vec::new();
    for seg in ns_node.path().iter() {
        if let Node::Symbol(sym) = seg {
            path.push(env.interner.symbols.intern(sym.as_str()));
        }
    }
    let name_sym_node = node.name();
    let name = env.interner.symbols.intern(name_sym_node.as_str());
    let ns = env.interner.namespaces.intern(&path, absolute);
    let kind = TypeNameKind::detect(name_sym_node.as_str());
    env.interner.intern(ns, name, kind)
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
            let inner = intern_type_name(env, &c.name());
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
            let inner = intern_type_name(env, &m.name());
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
            let inner = intern_type_name(env, &i.name());
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
            let inner = intern_type_name(env, &a.name());
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
            let inner = intern_type_name(env, &c.name());
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
            let inner = intern_type_name(env, &ca.new_name());
            let name = env.interner.with_prefix(namespace, inner);
            let old_name = intern_type_name(env, &ca.old_name());
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
            let inner = intern_type_name(env, &ma.new_name());
            let name = env.interner.with_prefix(namespace, inner);
            let old_name = intern_type_name(env, &ma.old_name());
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

fn is_decl_node(n: &Node<'_>) -> bool {
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
