//! Declaration walker: turns one source's top-level `NodeList` into
//! `Array[RBS::AST::Declarations::*]` Ruby values.
//!
//! Public entry point is [`materialize_declarations`]. Internally:
//!
//! - Top-level dispatch ([`materialize_top_decl_node`]) maps each
//!   AST node variant to one of the per-AST-node materialisers
//!   (`materialize_class_node`, `materialize_module_node`, …).
//! - Nested decls inside class / module members reach the same
//!   per-AST-node materialisers via [`materialize_nested_decl`].
//!
//! Each node materialiser anchors `ctx`'s resolution cursor (via
//! `enter_decl`) before recursing into bodies, so per-decl
//! [`ResolvedRef`] slices line up with the resolver driver's
//! pre-order walk.
//!
//! The entry-driven materialiser (`build_entries` / `process_*` /
//! per-entry wrappers) was retired in M3k Y3 — `add_source` on the
//! Ruby side now handles `*_decls` indexing.

use magnus::{Error, Value, kwargs, prelude::*, value::ReprValue};

use librbs_core::env::DeclRef;
use librbs_core::env::insert::{find_type_name_node, is_decl_node};
use librbs_core::interner::{NamespaceSym, TypeNameSym};
use ruby_rbs::node::{
    ClassAliasNode, ClassNode, ClassSuperNode, ConstantNode, GlobalNode, InterfaceNode,
    ModuleAliasNode, ModuleNode, ModuleSelfNode, Node, NodeList, TypeAliasNode,
};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{add_optional_child, add_required_child, make_location};
use crate::materialize::member::{build_annotations, build_comment, materialize_member};
use crate::materialize::type_::materialize_type;
use crate::materialize::type_name::{materialize_resolved_type_name, materialize_type_name};
use crate::materialize::type_param::materialize_type_params;

/// Walk a source's top-level declarations and materialise each into a
/// Ruby `RBS::AST::Declarations::*` instance, returning the resulting
/// `Array`. Resolution-cursor anchoring follows the same pre-order
/// (`is_decl_node` filtered) the resolver driver uses, so per-decl
/// `ResolvedRef` slices line up exactly. Non-decl nodes (the parser
/// shouldn't surface any in the declarations list, but the loop is
/// defensive) are filtered out via `is_decl_node`.
pub fn materialize_declarations(
    ctx: &mut MaterializeCtx<'_>,
    declarations: NodeList<'_>,
) -> Result<Value, Error> {
    let arr = ctx.ruby.ary_new();
    let root_ns = ctx.interner.namespaces().root_absolute();
    let mut counter: u32 = 0;
    for decl in declarations.iter() {
        if !is_decl_node(&decl) {
            continue;
        }
        let decl_index = counter;
        counter += 1;
        let decl_ref = DeclRef {
            source_index: ctx.source_index,
            decl_index,
        };
        let saved = ctx.save_cursor();
        ctx.enter_decl(decl_ref);
        let v = materialize_top_decl_node(ctx, &decl, root_ns, &mut counter)?;
        ctx.restore_cursor(saved);
        arr.push(v)?;
    }
    Ok(arr.as_value())
}

/// Dispatch one top-level decl AST node to the matching per-AST
/// materialiser. Every top-level decl's "full name" is anchored at
/// `Namespace.root`; nested decls are handled by the per-AST-node
/// materialisers internally (via `materialize_nested_decl`).
fn materialize_top_decl_node(
    ctx: &mut MaterializeCtx<'_>,
    node: &Node<'_>,
    root_ns: NamespaceSym,
    counter: &mut u32,
) -> Result<Value, Error> {
    match node {
        Node::Class(c) => {
            let full_name = full_decl_name(ctx, &c.name(), root_ns);
            let inner_ns = ctx
                .interner
                .to_namespace(full_name)
                .expect("class namespace pre-interned by insert");
            materialize_class_node(ctx, full_name, c, inner_ns, counter)
        }
        Node::Module(m) => {
            let full_name = full_decl_name(ctx, &m.name(), root_ns);
            let inner_ns = ctx
                .interner
                .to_namespace(full_name)
                .expect("module namespace pre-interned by insert");
            materialize_module_node(ctx, full_name, m, inner_ns, counter)
        }
        Node::Interface(i) => {
            let full_name = full_decl_name(ctx, &i.name(), root_ns);
            materialize_interface_node(ctx, full_name, i)
        }
        Node::TypeAlias(a) => {
            let full_name = full_decl_name(ctx, &a.name(), root_ns);
            materialize_type_alias_node(ctx, full_name, a)
        }
        Node::Constant(c) => {
            let full_name = full_decl_name(ctx, &c.name(), root_ns);
            materialize_constant_node(ctx, full_name, c)
        }
        Node::Global(g) => materialize_global_node(ctx, g),
        Node::ClassAlias(a) => {
            let full_new = full_decl_name(ctx, &a.new_name(), root_ns);
            materialize_class_alias_node(ctx, full_new, a)
        }
        Node::ModuleAlias(a) => {
            let full_new = full_decl_name(ctx, &a.new_name(), root_ns);
            materialize_module_alias_node(ctx, full_new, a)
        }
        _ => unreachable!("is_decl_node filtered to decl-only nodes"),
    }
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
/// module. The parent's `members` array carries
/// `RBS::AST::Declarations::*` instances (the same Ruby objects
/// upstream `add_source` then registers as the matching `*_decls`
/// entry's `decl`, preserving the cross-path identity invariant). The
/// local `counter` mirrors `insert_decl`'s pre-order numbering so
/// nested decls pull the right resolution slice.
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
///   net effect is the absolute form. Materialization with a
///   resolution side-table reproduces that rewrite by using
///   `full_name` directly.
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
