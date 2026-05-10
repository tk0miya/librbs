//! M3h: build Ruby `RBS::Environment::*Entry` instances and the six
//! `*_decls` Ruby Hashes by walking `librbs_core::Environment`'s
//! already-populated `*_decls` HashMaps. The AST scan happened at
//! insert time; the resolver pass produced a per-decl `Resolution`
//! side-table. This module's job is to combine those two structures
//! into upstream-compatible Ruby values — no second pre-order AST
//! traversal is performed.
//!
//! Each entry's `DeclRef` points back into the source's AST tree;
//! `librbs_core::resolver::driver::lookup_decl` retrieves the original
//! `ruby_rbs::node::Node` so the materializer can walk that single
//! decl and produce `RBS::AST::Declarations::*` instances.
//! `MaterializeCtx::enter_decl` anchors the resolution cursor before
//! each decl's body walk, so materialized type names match what
//! `resolve_type_names` recorded during resolution.

use magnus::{Error, IntoValue, RHash, Value, kwargs, prelude::*, value::ReprValue};

use librbs_core::Source;
use librbs_core::env::entry::{ClassAliasLikeEntry, ClassLikeEntry, Context, DeclRef};
use librbs_core::env::insert::{find_type_name_node, is_decl_node};
use librbs_core::interner::{NamespaceSym, Sym, TypeNameSym};
use librbs_core::resolver::driver::lookup_decl;
use ruby_rbs::node::{
    ClassAliasNode, ClassNode, ClassSuperNode, ConstantNode, GlobalNode, InterfaceNode,
    ModuleAliasNode, ModuleNode, ModuleSelfNode, Node, TypeAliasNode,
};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{add_optional_child, add_required_child, make_location};
use crate::materialize::member::{build_annotations, build_comment, materialize_member};
use crate::materialize::type_::materialize_type;
use crate::materialize::type_name::{materialize_resolved_type_name, materialize_type_name};
use crate::materialize::type_param::materialize_type_params;

/// Ruby-side handles to the six `*_decls` hashes populated by
/// [`build_entries`]. The caller assigns these onto the
/// `RBS::Environment` ivars upstream readers expect.
pub struct EntryHashes {
    pub class_decls: RHash,
    pub interface_decls: RHash,
    pub type_alias_decls: RHash,
    pub constant_decls: RHash,
    pub class_alias_decls: RHash,
    pub global_decls: RHash,
}

/// Walk every entry in `env.*_decls` and build the matching Ruby
/// `*_decls` hash. The Rust env is the source of truth for
/// (name, context, DeclRef) — we look the AST node up via DeclRef and
/// materialize ONLY that decl's body, no enclosing pre-order
/// re-traversal.
pub fn build_entries(ctx: &mut MaterializeCtx<'_>) -> Result<EntryHashes, Error> {
    let ruby = ctx.ruby;
    let hashes = EntryHashes {
        class_decls: ruby.hash_new(),
        interface_decls: ruby.hash_new(),
        type_alias_decls: ruby.hash_new(),
        constant_decls: ruby.hash_new(),
        class_alias_decls: ruby.hash_new(),
        global_decls: ruby.hash_new(),
    };

    // Snapshot each entry hash up front so iteration doesn't keep a
    // shared borrow on `ctx.env` while we mutate `ctx` (set_source /
    // enter_decl) for each decl. `Context` clones a small Vec<u32>
    // and `DeclRef` is `Copy`, so the snapshots are cheap.
    let class_snapshots: Vec<ClassLikeSnapshot> = ctx
        .env
        .class_decls
        .iter()
        .map(|(name, e)| ClassLikeSnapshot {
            name: *name,
            is_class: e.is_class(),
            context_decls: match e {
                ClassLikeEntry::Class(c) => c.context_decls.clone(),
                ClassLikeEntry::Module(m) => m.context_decls.clone(),
            },
        })
        .collect();
    for snap in class_snapshots {
        process_class_like(ctx, &hashes.class_decls, snap)?;
    }

    let interface_snapshots: Vec<SingleSnapshot> = ctx
        .env
        .interface_decls
        .iter()
        .map(|(name, e)| SingleSnapshot {
            name: *name,
            context: e.context.clone(),
            decl: e.decl,
        })
        .collect();
    for snap in interface_snapshots {
        process_interface(ctx, &hashes.interface_decls, snap)?;
    }

    let type_alias_snapshots: Vec<SingleSnapshot> = ctx
        .env
        .type_alias_decls
        .iter()
        .map(|(name, e)| SingleSnapshot {
            name: *name,
            context: e.context.clone(),
            decl: e.decl,
        })
        .collect();
    for snap in type_alias_snapshots {
        process_type_alias(ctx, &hashes.type_alias_decls, snap)?;
    }

    let constant_snapshots: Vec<SingleSnapshot> = ctx
        .env
        .constant_decls
        .iter()
        .map(|(name, e)| SingleSnapshot {
            name: *name,
            context: e.context.clone(),
            decl: e.decl,
        })
        .collect();
    for snap in constant_snapshots {
        process_constant(ctx, &hashes.constant_decls, snap)?;
    }

    let class_alias_snapshots: Vec<ClassAliasSnapshot> = ctx
        .env
        .class_alias_decls
        .values()
        .map(|e| ClassAliasSnapshot {
            name: e.name(),
            old_name: e.old_name(),
            is_class: matches!(e, ClassAliasLikeEntry::Class(_)),
            context: e.context().clone(),
            decl: match e {
                ClassAliasLikeEntry::Class(c) => c.decl,
                ClassAliasLikeEntry::Module(m) => m.decl,
            },
        })
        .collect();
    for snap in class_alias_snapshots {
        process_class_alias(ctx, &hashes.class_alias_decls, snap)?;
    }

    let global_snapshots: Vec<GlobalSnapshot> = ctx
        .env
        .global_decls
        .iter()
        .map(|(name, e)| GlobalSnapshot {
            name: *name,
            context: e.context.clone(),
            decl: e.decl,
        })
        .collect();
    for snap in global_snapshots {
        process_global(ctx, &hashes.global_decls, snap)?;
    }

    Ok(hashes)
}

struct ClassLikeSnapshot {
    name: TypeNameSym,
    is_class: bool,
    context_decls: Vec<(Context, DeclRef)>,
}

struct SingleSnapshot {
    name: TypeNameSym,
    context: Context,
    decl: DeclRef,
}

struct ClassAliasSnapshot {
    name: TypeNameSym,
    old_name: TypeNameSym,
    is_class: bool,
    context: Context,
    decl: DeclRef,
}

struct GlobalSnapshot {
    name: Sym,
    context: Context,
    decl: DeclRef,
}

// ---------- per-entry-kind processors ----------

fn process_class_like(
    ctx: &mut MaterializeCtx<'_>,
    hash: &RHash,
    snap: ClassLikeSnapshot,
) -> Result<(), Error> {
    let ruby_name = materialize_type_name(ctx, snap.name)?;
    let entry_class = if snap.is_class {
        ctx.classes.entry_class
    } else {
        ctx.classes.entry_module
    };
    let ruby_entry = entry_class.new_instance((ruby_name,))?.as_value();

    let my_ns = ctx
        .interner
        .to_namespace(snap.name)
        .expect("class/module namespace pre-interned by insert");

    for (rust_ctx, decl_ref) in snap.context_decls {
        let ruby_ctx = build_ruby_context(ctx, &rust_ctx)?;
        let ruby_decl = materialize_class_or_module_decl(ctx, snap.name, decl_ref, my_ns)?;
        let pair = ctx.ruby.ary_new_capa(2);
        pair.push(ruby_ctx)?;
        pair.push(ruby_decl)?;
        let _: Value = ruby_entry.funcall("<<", (pair.as_value().into_value_with(ctx.ruby),))?;
    }

    hash.aset(ruby_name, ruby_entry)?;
    Ok(())
}

fn process_interface(
    ctx: &mut MaterializeCtx<'_>,
    hash: &RHash,
    snap: SingleSnapshot,
) -> Result<(), Error> {
    let ruby_name = materialize_type_name(ctx, snap.name)?;
    let ruby_ctx = build_ruby_context(ctx, &snap.context)?;
    let ruby_decl = materialize_single_decl(ctx, snap.name, snap.decl, NodeKind::Interface)?;
    let entry = ctx
        .classes
        .entry_interface
        .new_instance((kwargs!(
            "name" => ruby_name,
            "decl" => ruby_decl,
            "context" => ruby_ctx
        ),))?
        .as_value();
    hash.aset(ruby_name, entry)?;
    Ok(())
}

fn process_type_alias(
    ctx: &mut MaterializeCtx<'_>,
    hash: &RHash,
    snap: SingleSnapshot,
) -> Result<(), Error> {
    let ruby_name = materialize_type_name(ctx, snap.name)?;
    let ruby_ctx = build_ruby_context(ctx, &snap.context)?;
    let ruby_decl = materialize_single_decl(ctx, snap.name, snap.decl, NodeKind::TypeAlias)?;
    let entry = ctx
        .classes
        .entry_type_alias
        .new_instance((kwargs!(
            "name" => ruby_name,
            "decl" => ruby_decl,
            "context" => ruby_ctx
        ),))?
        .as_value();
    hash.aset(ruby_name, entry)?;
    Ok(())
}

fn process_constant(
    ctx: &mut MaterializeCtx<'_>,
    hash: &RHash,
    snap: SingleSnapshot,
) -> Result<(), Error> {
    let ruby_name = materialize_type_name(ctx, snap.name)?;
    let ruby_ctx = build_ruby_context(ctx, &snap.context)?;
    let ruby_decl = materialize_single_decl(ctx, snap.name, snap.decl, NodeKind::Constant)?;
    let entry = ctx
        .classes
        .entry_constant
        .new_instance((kwargs!(
            "name" => ruby_name,
            "decl" => ruby_decl,
            "context" => ruby_ctx
        ),))?
        .as_value();
    hash.aset(ruby_name, entry)?;
    Ok(())
}

fn process_class_alias(
    ctx: &mut MaterializeCtx<'_>,
    hash: &RHash,
    snap: ClassAliasSnapshot,
) -> Result<(), Error> {
    let ruby_name = materialize_type_name(ctx, snap.name)?;
    let ruby_ctx = build_ruby_context(ctx, &snap.context)?;
    let kind = if snap.is_class {
        NodeKind::ClassAlias
    } else {
        NodeKind::ModuleAlias
    };
    let ruby_decl = materialize_single_decl(ctx, snap.name, snap.decl, kind)?;
    let entry_class = if snap.is_class {
        ctx.classes.entry_class_alias
    } else {
        ctx.classes.entry_module_alias
    };
    let entry = entry_class
        .new_instance((kwargs!(
            "name" => ruby_name,
            "decl" => ruby_decl,
            "context" => ruby_ctx
        ),))?
        .as_value();
    hash.aset(ruby_name, entry)?;
    // `old_name` is only used by the resolver / type checker through
    // the decl itself; the entry exposes it via `entry.decl.old_name`.
    let _ = snap.old_name;
    Ok(())
}

fn process_global(
    ctx: &mut MaterializeCtx<'_>,
    hash: &RHash,
    snap: GlobalSnapshot,
) -> Result<(), Error> {
    let name_str = ctx.interner.symbols().lookup(snap.name).to_string();
    let ruby_name = ctx.ruby.to_symbol(&name_str).as_value();
    let ruby_ctx = build_ruby_context(ctx, &snap.context)?;
    // Globals key by Symbol, not TypeNameSym, so they go straight
    // to `materialize_global_node` rather than through the
    // TypeName-shaped `materialize_single_decl` dispatcher.
    ctx.set_source(snap.decl.source_index);
    ctx.enter_decl(snap.decl);
    let src = source_ref(ctx, snap.decl.source_index);
    let ast = lookup_decl(src, snap.decl).expect("global decl_ref points to a real decl");
    let Node::Global(g) = ast else {
        unreachable!("global entry decl_ref does not point to a global");
    };
    let ruby_decl = materialize_global_node(ctx, &g)?;
    let entry = ctx
        .classes
        .entry_global
        .new_instance((kwargs!(
            "name" => ruby_name,
            "decl" => ruby_decl,
            "context" => ruby_ctx
        ),))?
        .as_value();
    hash.aset(ruby_name, entry)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum NodeKind {
    Interface,
    TypeAlias,
    Constant,
    ClassAlias,
    ModuleAlias,
}

/// Look the AST node up via `decl_ref`, anchor the resolution cursor,
/// and dispatch to the per-variant builder. Used by SingleEntry-style
/// decls (interfaces, type aliases, constants, globals, aliases),
/// which never contain nested decls so the local counter stays unused.
///
/// Each per-AST-node materializer derives the decl's own `name` from
/// its literal `name_node` so the resulting Ruby decl preserves the
/// source-form (relative vs. absolute) the user wrote, matching pure
/// RBS. The entry-key absolute name is computed separately by the
/// caller (`process_*`).
fn materialize_single_decl(
    ctx: &mut MaterializeCtx<'_>,
    full_name: TypeNameSym,
    decl_ref: DeclRef,
    kind: NodeKind,
) -> Result<Value, Error> {
    ctx.set_source(decl_ref.source_index);
    ctx.enter_decl(decl_ref);
    let src = source_ref(ctx, decl_ref.source_index);
    let ast = lookup_decl(src, decl_ref).expect("entry decl_ref points to a real decl");
    match (kind, ast) {
        (NodeKind::Interface, Node::Interface(i)) => materialize_interface_node(ctx, full_name, &i),
        (NodeKind::TypeAlias, Node::TypeAlias(a)) => {
            materialize_type_alias_node(ctx, full_name, &a)
        }
        (NodeKind::Constant, Node::Constant(c)) => materialize_constant_node(ctx, full_name, &c),
        (NodeKind::ClassAlias, Node::ClassAlias(a)) => {
            materialize_class_alias_node(ctx, full_name, &a)
        }
        (NodeKind::ModuleAlias, Node::ModuleAlias(a)) => {
            materialize_module_alias_node(ctx, full_name, &a)
        }
        _ => unreachable!("entry kind/AST node mismatch — env::insert invariant violated"),
    }
}

/// Class/Module variant of [`materialize_single_decl`]. Maintains a
/// local pre-order counter (starting at this decl's index + 1) so any
/// nested decl members can be assigned the same `decl_index` insert.rs
/// gave them, and thus pull the right resolution slice via
/// `enter_decl`.
fn materialize_class_or_module_decl(
    ctx: &mut MaterializeCtx<'_>,
    full_name: TypeNameSym,
    decl_ref: DeclRef,
    my_namespace: NamespaceSym,
) -> Result<Value, Error> {
    ctx.set_source(decl_ref.source_index);
    ctx.enter_decl(decl_ref);
    let src = source_ref(ctx, decl_ref.source_index);
    let ast = lookup_decl(src, decl_ref).expect("entry decl_ref points to a real decl");
    let mut counter = decl_ref.decl_index + 1;
    match ast {
        Node::Class(c) => materialize_class_node(ctx, full_name, &c, my_namespace, &mut counter),
        Node::Module(m) => materialize_module_node(ctx, full_name, &m, my_namespace, &mut counter),
        _ => unreachable!("class entry decl_ref does not point to class/module"),
    }
}

/// Borrow the source at `index` with a lifetime that does not flow back
/// into `ctx`. SAFETY: `env.sources` is never resized after
/// `from_loader` returns; the parser data the returned `Source` points
/// at is heap-stable. The same `&*src_ptr` trick is used in
/// `resolver::driver::resolve` for the same reason.
fn source_ref<'a>(ctx: &MaterializeCtx<'a>, index: u32) -> &'a Source {
    let ptr: *const Source = &ctx.env.sources[index as usize];
    unsafe { &*ptr }
}

// ---------- per-AST-node materializers ----------

fn materialize_class_node(
    ctx: &mut MaterializeCtx<'_>,
    full_name: TypeNameSym,
    node: &ClassNode<'_>,
    my_namespace: NamespaceSym,
    counter: &mut u32,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;
    add_required_child(ctx, loc, "name", &node.name_location())?;
    add_required_child(ctx, loc, "end", &node.end_location())?;
    add_optional_child(
        ctx,
        loc,
        "type_params",
        node.type_params_location().as_ref(),
    )?;
    add_optional_child(ctx, loc, "lt", node.lt_location().as_ref())?;

    let ruby_name = decl_self_name(ctx, &node.name(), full_name)?;

    // Order must match `walk_class` in the resolver driver: type_params
    // first (advance cursor through bound types), then super_class
    // (its name is the next ResolvedRef + any args), then members.
    let type_params = materialize_type_params(ctx, node.type_params())?;
    let super_class: Value = match node.super_class() {
        Some(sc) => class_super(ctx, &sc)?,
        None => ctx.ruby.qnil().as_value(),
    };

    let members = ctx.ruby.ary_new();
    for member in node.members().iter() {
        if is_decl_node(&member) {
            let nested = materialize_nested_decl(ctx, &member, my_namespace, counter)?;
            members.push(nested)?;
        } else {
            members.push(materialize_member(ctx, &member)?)?;
        }
    }

    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .decls_class
        .new_instance((kwargs!(
            "name" => ruby_name,
            "type_params" => type_params,
            "super_class" => super_class,
            "members" => members,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn materialize_module_node(
    ctx: &mut MaterializeCtx<'_>,
    full_name: TypeNameSym,
    node: &ModuleNode<'_>,
    my_namespace: NamespaceSym,
    counter: &mut u32,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;
    add_required_child(ctx, loc, "name", &node.name_location())?;
    add_required_child(ctx, loc, "end", &node.end_location())?;
    add_optional_child(
        ctx,
        loc,
        "type_params",
        node.type_params_location().as_ref(),
    )?;
    add_optional_child(ctx, loc, "colon", node.colon_location().as_ref())?;
    add_optional_child(ctx, loc, "self_types", node.self_types_location().as_ref())?;

    let ruby_name = decl_self_name(ctx, &node.name(), full_name)?;
    let type_params = materialize_type_params(ctx, node.type_params())?;

    let self_types = ctx.ruby.ary_new();
    for st in node.self_types().iter() {
        let Node::ModuleSelf(ms) = &st else {
            unreachable!("module self_types holds ModuleSelf nodes only");
        };
        self_types.push(module_self(ctx, ms)?)?;
    }

    let members = ctx.ruby.ary_new();
    for member in node.members().iter() {
        if is_decl_node(&member) {
            let nested = materialize_nested_decl(ctx, &member, my_namespace, counter)?;
            members.push(nested)?;
        } else {
            members.push(materialize_member(ctx, &member)?)?;
        }
    }

    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .decls_module
        .new_instance((kwargs!(
            "name" => ruby_name,
            "type_params" => type_params,
            "members" => members,
            "self_types" => self_types,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn materialize_interface_node(
    ctx: &mut MaterializeCtx<'_>,
    full_name: TypeNameSym,
    node: &InterfaceNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;
    add_required_child(ctx, loc, "name", &node.name_location())?;
    add_required_child(ctx, loc, "end", &node.end_location())?;
    add_optional_child(
        ctx,
        loc,
        "type_params",
        node.type_params_location().as_ref(),
    )?;

    let ruby_name = decl_self_name(ctx, &node.name(), full_name)?;
    let type_params = materialize_type_params(ctx, node.type_params())?;

    let members = ctx.ruby.ary_new();
    for m in node.members().iter() {
        members.push(materialize_member(ctx, &m)?)?;
    }

    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .decls_interface
        .new_instance((kwargs!(
            "name" => ruby_name,
            "type_params" => type_params,
            "members" => members,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn materialize_type_alias_node(
    ctx: &mut MaterializeCtx<'_>,
    full_name: TypeNameSym,
    node: &TypeAliasNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;
    add_required_child(ctx, loc, "name", &node.name_location())?;
    add_required_child(ctx, loc, "eq", &node.eq_location())?;
    add_optional_child(
        ctx,
        loc,
        "type_params",
        node.type_params_location().as_ref(),
    )?;

    let ruby_name = decl_self_name(ctx, &node.name(), full_name)?;
    let type_params = materialize_type_params(ctx, node.type_params())?;
    let target_node = node.type_();
    let ty = materialize_type(ctx, &target_node)?;
    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .decls_type_alias
        .new_instance((kwargs!(
            "name" => ruby_name,
            "type_params" => type_params,
            "type" => ty,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn materialize_constant_node(
    ctx: &mut MaterializeCtx<'_>,
    full_name: TypeNameSym,
    node: &ConstantNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "name", &node.name_location())?;
    add_required_child(ctx, loc, "colon", &node.colon_location())?;

    let ruby_name = decl_self_name(ctx, &node.name(), full_name)?;
    let target_node = node.type_();
    let ty = materialize_type(ctx, &target_node)?;
    let comment = build_comment(ctx, node.comment())?;
    let annotations = build_annotations(ctx, node.annotations())?;
    Ok(ctx
        .classes
        .decls_constant
        .new_instance((kwargs!(
            "name" => ruby_name,
            "type" => ty,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn materialize_global_node(
    ctx: &mut MaterializeCtx<'_>,
    node: &GlobalNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "name", &node.name_location())?;
    add_required_child(ctx, loc, "colon", &node.colon_location())?;

    let ruby_name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    let target_node = node.type_();
    let ty = materialize_type(ctx, &target_node)?;
    let comment = build_comment(ctx, node.comment())?;
    let annotations = build_annotations(ctx, node.annotations())?;
    Ok(ctx
        .classes
        .decls_global
        .new_instance((kwargs!(
            "name" => ruby_name,
            "type" => ty,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn materialize_class_alias_node(
    ctx: &mut MaterializeCtx<'_>,
    full_new_name: TypeNameSym,
    node: &ClassAliasNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;
    add_required_child(ctx, loc, "new_name", &node.new_name_location())?;
    add_required_child(ctx, loc, "eq", &node.eq_location())?;
    add_required_child(ctx, loc, "old_name", &node.old_name_location())?;

    let ruby_new_name = decl_self_name(ctx, &node.new_name(), full_new_name)?;
    let raw_old = find_type_name_node(ctx.interner, &node.old_name())
        .expect("alias old_name pre-interned by insert");
    let old_name_v = materialize_resolved_type_name(ctx, raw_old)?;

    let comment = build_comment(ctx, node.comment())?;
    let annotations = build_annotations(ctx, node.annotations())?;
    Ok(ctx
        .classes
        .decls_class_alias
        .new_instance((kwargs!(
            "new_name" => ruby_new_name,
            "old_name" => old_name_v,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn materialize_module_alias_node(
    ctx: &mut MaterializeCtx<'_>,
    full_new_name: TypeNameSym,
    node: &ModuleAliasNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;
    add_required_child(ctx, loc, "new_name", &node.new_name_location())?;
    add_required_child(ctx, loc, "eq", &node.eq_location())?;
    add_required_child(ctx, loc, "old_name", &node.old_name_location())?;

    let ruby_new_name = decl_self_name(ctx, &node.new_name(), full_new_name)?;
    let raw_old = find_type_name_node(ctx.interner, &node.old_name())
        .expect("alias old_name pre-interned by insert");
    let old_name_v = materialize_resolved_type_name(ctx, raw_old)?;

    let comment = build_comment(ctx, node.comment())?;
    let annotations = build_annotations(ctx, node.annotations())?;
    Ok(ctx
        .classes
        .decls_module_alias
        .new_instance((kwargs!(
            "new_name" => ruby_new_name,
            "old_name" => old_name_v,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

/// Materialize a decl that appears as a member of a parent class /
/// module. The parent's `members` array needs a `RBS::AST::Declarations::*`
/// instance (separate Ruby object from the one in `*_decls`'s entry,
/// but content-equal). The local `counter` mirrors `insert_decl`'s
/// pre-order numbering so nested decls pull the right resolution slice.
fn materialize_nested_decl(
    ctx: &mut MaterializeCtx<'_>,
    member: &Node<'_>,
    parent_namespace: NamespaceSym,
    counter: &mut u32,
) -> Result<Value, Error> {
    let nested_decl_index = *counter;
    *counter += 1;
    let nested_decl_ref = DeclRef {
        source_index: ctx.source_index,
        decl_index: nested_decl_index,
    };

    let saved = ctx.save_cursor();
    ctx.enter_decl(nested_decl_ref);

    // Compute the absolute full name once per nested decl. Pure-RBS
    // resolve_type_names rewrites `decl.name` to this form via
    // `with_prefix`; `decl_self_name` consults `ctx.resolution` and
    // chooses between this absolute form (resolved env) and the
    // literal source form (unresolved env).
    let result = match member {
        Node::Class(c) => {
            let full_name = full_decl_name(ctx, &c.name(), parent_namespace);
            let inner_ns = ctx
                .interner
                .to_namespace(full_name)
                .expect("nested class namespace pre-interned by insert");
            materialize_class_node(ctx, full_name, c, inner_ns, counter)
        }
        Node::Module(m) => {
            let full_name = full_decl_name(ctx, &m.name(), parent_namespace);
            let inner_ns = ctx
                .interner
                .to_namespace(full_name)
                .expect("nested module namespace pre-interned by insert");
            materialize_module_node(ctx, full_name, m, inner_ns, counter)
        }
        Node::Interface(i) => {
            let full_name = full_decl_name(ctx, &i.name(), parent_namespace);
            materialize_interface_node(ctx, full_name, i)
        }
        Node::TypeAlias(a) => {
            let full_name = full_decl_name(ctx, &a.name(), parent_namespace);
            materialize_type_alias_node(ctx, full_name, a)
        }
        Node::Constant(c) => {
            let full_name = full_decl_name(ctx, &c.name(), parent_namespace);
            materialize_constant_node(ctx, full_name, c)
        }
        Node::Global(g) => materialize_global_node(ctx, g),
        Node::ClassAlias(a) => {
            let full_new_name = full_decl_name(ctx, &a.new_name(), parent_namespace);
            materialize_class_alias_node(ctx, full_new_name, a)
        }
        Node::ModuleAlias(a) => {
            let full_new_name = full_decl_name(ctx, &a.new_name(), parent_namespace);
            materialize_module_alias_node(ctx, full_new_name, a)
        }
        _ => unreachable!("materialize_nested_decl called on non-decl"),
    };

    ctx.restore_cursor(saved);
    result
}

// ---------- helpers ----------

/// Compute the absolute `TypeNameSym` for a decl's name node by
/// looking up its pre-interned inner symbol and prepending the parent
/// namespace via `FrozenInterner::with_prefix`. The combination is
/// guaranteed to exist because M2 `insert_decl` interned every
/// declaration's full name through the same path.
fn full_decl_name(
    ctx: &MaterializeCtx<'_>,
    name_node: &ruby_rbs::node::TypeNameNode<'_>,
    parent_namespace: NamespaceSym,
) -> TypeNameSym {
    let inner =
        find_type_name_node(ctx.interner, name_node).expect("decl name pre-interned by insert");
    ctx.interner
        .with_prefix(parent_namespace, inner)
        .expect("absolute decl name pre-interned by insert")
}

/// Build the Ruby `RBS::TypeName` for a decl's *own* name. Mirrors
/// upstream's two-phase handling:
///
/// - Unresolved env (no `Resolution` attached): `decl.name` keeps the
///   source form (relative if the source did not write `::Foo`),
///   matching pure RBS's parser output verbatim.
/// - Resolved env: `RBS::Environment#resolve_type_names` walks every
///   decl and rewrites its `name` via `with_prefix(prefix)` — the
///   net effect is the entry-key absolute form. Materialization with
///   a resolution side-table reproduces that rewrite by using
///   `full_name` directly.
///
/// This is separate from the entry-key absolute name that
/// `class_decls` is keyed by — the entry key is always absolute on
/// both sides and is computed by the caller from `snap.name` /
/// `full_decl_name`.
fn decl_self_name(
    ctx: &MaterializeCtx<'_>,
    name_node: &ruby_rbs::node::TypeNameNode<'_>,
    full_name: TypeNameSym,
) -> Result<Value, Error> {
    let sym = if ctx.resolution.is_some() {
        full_name
    } else {
        find_type_name_node(ctx.interner, name_node).expect("decl name pre-interned by insert")
    };
    materialize_type_name(ctx, sym)
}

/// Convert `Context = Vec<TypeNameSym>` into upstream's nil-cons-cell
/// linked list. Top-level decls have an empty `Vec`, which maps to
/// `nil`. `[Foo]` maps to `[nil, ::Foo]`. `[Foo, Bar]` maps to
/// `[[nil, ::Foo], ::Foo::Bar]`. Outer-to-inner order.
fn build_ruby_context(ctx: &MaterializeCtx<'_>, rust_ctx: &Context) -> Result<Value, Error> {
    let mut acc = ctx.ruby.qnil().as_value();
    for sym in rust_ctx {
        let name = materialize_type_name(ctx, *sym)?;
        let pair = ctx.ruby.ary_new_capa(2);
        pair.push(acc)?;
        pair.push(name)?;
        acc = pair.as_value();
    }
    Ok(acc)
}

fn class_super(ctx: &mut MaterializeCtx<'_>, sc: &ClassSuperNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &sc.location())?;
    add_required_child(ctx, loc, "name", &sc.name_location())?;
    add_optional_child(ctx, loc, "args", sc.args_location().as_ref())?;

    let raw = find_type_name_node(ctx.interner, &sc.name())
        .expect("super class name pre-interned by insert");
    let name = materialize_resolved_type_name(ctx, raw)?;
    let args = ctx.ruby.ary_new();
    for a in sc.args().iter() {
        args.push(materialize_type(ctx, &a)?)?;
    }
    Ok(ctx
        .classes
        .decls_class_super
        .new_instance((kwargs!(
            "name" => name,
            "args" => args,
            "location" => loc
        ),))?
        .as_value())
}

fn module_self(ctx: &mut MaterializeCtx<'_>, ms: &ModuleSelfNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &ms.location())?;
    add_required_child(ctx, loc, "name", &ms.name_location())?;
    add_optional_child(ctx, loc, "args", ms.args_location().as_ref())?;

    let raw = find_type_name_node(ctx.interner, &ms.name())
        .expect("module self-type name pre-interned by insert");
    let name = materialize_resolved_type_name(ctx, raw)?;
    let args = ctx.ruby.ary_new();
    for a in ms.args().iter() {
        args.push(materialize_type(ctx, &a)?)?;
    }
    Ok(ctx
        .classes
        .decls_module_self
        .new_instance((kwargs!(
            "name" => name,
            "args" => args,
            "location" => loc
        ),))?
        .as_value())
}
