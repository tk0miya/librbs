//! M3g: build `RBS::AST::Members::*` instances from `ruby_rbs::node`
//! member nodes. Annotation and Comment helpers (`build_annotation`,
//! `build_annotations`, `build_comment`) live here too — declarations
//! pull on them in M3h.
//!
//! [`materialize_member`] dispatch mirrors
//! `crates/librbs-core/src/resolver/driver.rs::walk_member` so the two
//! walks stay in lockstep with the resolution cursor on
//! [`MaterializeCtx`]: any `Include` / `Extend` / `Prepend` mixin name
//! and any nested type-name occurrence is pulled from the cursor in the
//! same pre-order the driver pushed it.

use magnus::{Error, IntoValue, RArray, Value, kwargs, prelude::*, value::ReprValue};

use librbs_core::env::insert::find_type_name_node;
use ruby_rbs::node::{
    AliasKind, AliasNode, AnnotationNode, AttrAccessorNode, AttrIvarName, AttrReaderNode,
    AttrWriterNode, AttributeKind, AttributeVisibility, ClassInstanceVariableNode,
    ClassVariableNode, CommentNode, ExtendNode, IncludeNode, InstanceVariableNode,
    MethodDefinitionKind, MethodDefinitionNode, MethodDefinitionVisibility, Node, PrependNode,
    PrivateNode, PublicNode, RBSLocationRange,
};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{add_optional_child, add_required_child, make_location};
use crate::materialize::method_type::materialize_method_type;
use crate::materialize::type_::materialize_type;
use crate::materialize::type_name::materialize_resolved_type_name;

/// Dispatch a member `Node` into the appropriate Ruby builder. Mirrors
/// `walk_member` in the resolver driver — every `RBS::AST::Members::*`
/// variant has its own arm, and unrelated `Node` variants panic so a
/// future parser change forces this list to be updated.
pub fn materialize_member(ctx: &mut MaterializeCtx<'_>, node: &Node<'_>) -> Result<Value, Error> {
    let _t = crate::materialize::phase_timer::PhaseTimer::new(
        crate::materialize::phase_timer::Phase::Member,
    );
    match node {
        Node::MethodDefinition(m) => method_definition(ctx, m),
        Node::AttrAccessor(a) => attr_accessor(ctx, a),
        Node::AttrReader(a) => attr_reader(ctx, a),
        Node::AttrWriter(a) => attr_writer(ctx, a),
        Node::InstanceVariable(v) => instance_variable(ctx, v),
        Node::ClassInstanceVariable(v) => class_instance_variable(ctx, v),
        Node::ClassVariable(v) => class_variable(ctx, v),
        Node::Include(m) => include_member(ctx, m),
        Node::Extend(m) => extend_member(ctx, m),
        Node::Prepend(m) => prepend_member(ctx, m),
        Node::Alias(a) => alias_member(ctx, a),
        Node::Public(p) => public_member(ctx, p),
        Node::Private(p) => private_member(ctx, p),
        // Listing the rest exhaustively would mean recompiling this
        // file every time the parser gets a new node variant. Panic
        // instead — this branch is unreachable as long as the caller
        // dispatches only on member nodes.
        Node::Annotation(_)
        | Node::Bool(_)
        | Node::Comment(_)
        | Node::Class(_)
        | Node::Module(_)
        | Node::Interface(_)
        | Node::TypeAlias(_)
        | Node::Constant(_)
        | Node::Global(_)
        | Node::ClassAlias(_)
        | Node::ModuleAlias(_)
        | Node::ClassSuper(_)
        | Node::ModuleSelf(_)
        | Node::MethodDefinitionOverload(_)
        | Node::TypeParam(_)
        | Node::Integer(_)
        | Node::String(_)
        | Node::MethodType(_)
        | Node::Namespace(_)
        | Node::Signature(_)
        | Node::TypeName(_)
        | Node::FunctionParam(_)
        | Node::RecordFieldType(_)
        | Node::Symbol(_)
        | Node::Use(_)
        | Node::UseSingleClause(_)
        | Node::UseWildcardClause(_)
        | Node::BoolType(_)
        | Node::VoidType(_)
        | Node::AnyType(_)
        | Node::NilType(_)
        | Node::TopType(_)
        | Node::BottomType(_)
        | Node::SelfType(_)
        | Node::InstanceType(_)
        | Node::ClassType(_)
        | Node::VariableType(_)
        | Node::LiteralType(_)
        | Node::ClassInstanceType(_)
        | Node::InterfaceType(_)
        | Node::AliasType(_)
        | Node::ClassSingletonType(_)
        | Node::TupleType(_)
        | Node::UnionType(_)
        | Node::IntersectionType(_)
        | Node::RecordType(_)
        | Node::OptionalType(_)
        | Node::ProcType(_)
        | Node::FunctionType(_)
        | Node::UntypedFunctionType(_)
        | Node::BlockType(_)
        | Node::NodeTypeAssertion(_)
        | Node::ColonMethodTypeAnnotation(_)
        | Node::MethodTypesAnnotation(_)
        | Node::SkipAnnotation(_)
        | Node::ReturnTypeAnnotation(_)
        | Node::TypeApplicationAnnotation(_)
        | Node::InstanceVariableAnnotation(_)
        | Node::ClassAliasAnnotation(_)
        | Node::ModuleAliasAnnotation(_)
        | Node::ParamTypeAnnotation(_)
        | Node::SplatParamTypeAnnotation(_)
        | Node::DoubleSplatParamTypeAnnotation(_)
        | Node::BlockParamTypeAnnotation(_) => {
            unreachable!("materialize_member called on non-member node variant; walk parity bug")
        }
    }
}

// ---------- Annotation / Comment (re-exported for M3h decls) ----------

/// `RBS::AST::Annotation.new(string:, location:)`.
pub fn build_annotation(
    ctx: &mut MaterializeCtx<'_>,
    node: &AnnotationNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let s = node.string().as_str().to_string();
    let s_v: Value = s.into_value_with(ctx.ruby);
    Ok(ctx
        .classes
        .annotation
        .new_instance((kwargs!("string" => s_v, "location" => loc),))?
        .as_value())
}

/// Materialize a list of annotations. Empty list → empty `Array`.
pub fn build_annotations(
    ctx: &mut MaterializeCtx<'_>,
    list: ruby_rbs::node::NodeList<'_>,
) -> Result<RArray, Error> {
    let arr = ctx.ruby.ary_new();
    for n in list.iter() {
        let Node::Annotation(a) = &n else {
            unreachable!("annotations list must hold Annotation nodes only");
        };
        arr.push(build_annotation(ctx, a)?)?;
    }
    Ok(arr)
}

/// `RBS::AST::Comment.new(string:, location:)`. Returns `nil` when the
/// member has no leading comment.
pub fn build_comment(
    ctx: &mut MaterializeCtx<'_>,
    comment: Option<CommentNode<'_>>,
) -> Result<Value, Error> {
    match comment {
        None => Ok(ctx.ruby.qnil().as_value()),
        Some(c) => {
            let loc = make_location(ctx, &c.location())?;
            let s = c.string().as_str().to_string();
            let s_v: Value = s.into_value_with(ctx.ruby);
            Ok(ctx
                .classes
                .comment
                .new_instance((kwargs!("string" => s_v, "location" => loc),))?
                .as_value())
        }
    }
}

// ---------- helpers ----------

fn method_definition_kind(ctx: &MaterializeCtx<'_>, kind: MethodDefinitionKind) -> Value {
    match kind {
        MethodDefinitionKind::Instance => ctx.ruby.to_symbol("instance").as_value(),
        MethodDefinitionKind::Singleton => ctx.ruby.to_symbol("singleton").as_value(),
        MethodDefinitionKind::SingletonInstance => {
            ctx.ruby.to_symbol("singleton_instance").as_value()
        }
    }
}

fn method_definition_visibility(
    ctx: &MaterializeCtx<'_>,
    vis: MethodDefinitionVisibility,
) -> Value {
    match vis {
        MethodDefinitionVisibility::Unspecified => ctx.ruby.qnil().as_value(),
        MethodDefinitionVisibility::Public => ctx.ruby.to_symbol("public").as_value(),
        MethodDefinitionVisibility::Private => ctx.ruby.to_symbol("private").as_value(),
    }
}

fn attribute_kind(ctx: &MaterializeCtx<'_>, kind: AttributeKind) -> Value {
    match kind {
        AttributeKind::Instance => ctx.ruby.to_symbol("instance").as_value(),
        AttributeKind::Singleton => ctx.ruby.to_symbol("singleton").as_value(),
    }
}

fn attribute_visibility(ctx: &MaterializeCtx<'_>, vis: AttributeVisibility) -> Value {
    match vis {
        AttributeVisibility::Unspecified => ctx.ruby.qnil().as_value(),
        AttributeVisibility::Public => ctx.ruby.to_symbol("public").as_value(),
        AttributeVisibility::Private => ctx.ruby.to_symbol("private").as_value(),
    }
}

fn alias_kind(ctx: &MaterializeCtx<'_>, kind: AliasKind) -> Value {
    match kind {
        AliasKind::Instance => ctx.ruby.to_symbol("instance").as_value(),
        AliasKind::Singleton => ctx.ruby.to_symbol("singleton").as_value(),
    }
}

/// Render `AttrIvarName` to its Ruby surface (`nil` / `false` / Symbol).
/// `Name(constant_id)` cannot be looked up through the public ruby-rbs
/// API, so we recover the symbol text from the ivar-name location's
/// byte range. Mirrors `ast_translation.c::rbs_attr_ivar_name_to_ruby`
/// — which reads the constant pool — but routes around the missing
/// pool accessor by going through the buffer.
fn attr_ivar_name(
    ctx: &MaterializeCtx<'_>,
    ivar: AttrIvarName,
    ivar_name_loc: Option<RBSLocationRange>,
) -> Value {
    match ivar {
        AttrIvarName::Unspecified => ctx.ruby.qnil().as_value(),
        AttrIvarName::Empty => ctx.ruby.qfalse().as_value(),
        AttrIvarName::Name(_) => {
            let range = ivar_name_loc
                .expect("AttrIvarName::Name requires the parser to record an ivar_name range");
            let content = ctx.env.sources[ctx.source_index as usize]
                .buffer
                .content
                .as_str();
            let start = range.start() as usize;
            let end = range.end() as usize;
            let text = &content[start..end];
            ctx.ruby.to_symbol(text).as_value()
        }
    }
}

fn build_args_array(
    ctx: &mut MaterializeCtx<'_>,
    args: ruby_rbs::node::NodeList<'_>,
) -> Result<RArray, Error> {
    let arr = ctx.ruby.ary_new();
    for a in args.iter() {
        arr.push(materialize_type(ctx, &a)?)?;
    }
    Ok(arr)
}

// ---------- members ----------

fn method_definition(
    ctx: &mut MaterializeCtx<'_>,
    node: &MethodDefinitionNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;
    add_required_child(ctx, loc, "name", &node.name_location())?;
    add_optional_child(ctx, loc, "kind", node.kind_location().as_ref())?;
    add_optional_child(
        ctx,
        loc,
        "overloading",
        node.overloading_location().as_ref(),
    )?;
    add_optional_child(ctx, loc, "visibility", node.visibility_location().as_ref())?;

    let name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    let kind = method_definition_kind(ctx, node.kind());
    let visibility = method_definition_visibility(ctx, node.visibility());

    let overloads = ctx.ruby.ary_new();
    for ov in node.overloads().iter() {
        let Node::MethodDefinitionOverload(o) = &ov else {
            unreachable!("MethodDefinition.overloads holds Overload nodes only");
        };
        let annotations = build_annotations(ctx, o.annotations())?;
        let mt_node = o.method_type();
        let Node::MethodType(mt) = &mt_node else {
            unreachable!("MethodDefinitionOverload.method_type must be MethodType");
        };
        let method_type = materialize_method_type(ctx, mt)?;
        let overload_class: magnus::RClass = ctx
            .classes
            .members_method_definition
            .funcall("const_get", (ctx.ruby.to_symbol("Overload"),))?;
        let overload = overload_class
            .new_instance((kwargs!(
                "method_type" => method_type,
                "annotations" => annotations
            ),))?
            .as_value();
        overloads.push(overload)?;
    }

    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;

    Ok(ctx
        .classes
        .members_method_definition
        .new_instance((kwargs!(
            "name" => name,
            "kind" => kind,
            "overloads" => overloads,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment,
            "overloading" => node.overloading(),
            "visibility" => visibility
        ),))?
        .as_value())
}

/// Shared location-builder input for `attr_reader` / `attr_accessor` /
/// `attr_writer`. The three attr nodes carry the same set of
/// keyword / name / colon / kind / ivar / ivar_name / visibility
/// ranges; bundling them into a struct keeps the helper signature
/// under clippy's `too_many_arguments` threshold.
struct AttrLocations {
    range: RBSLocationRange,
    keyword: RBSLocationRange,
    name: RBSLocationRange,
    colon: RBSLocationRange,
    kind: Option<RBSLocationRange>,
    ivar: Option<RBSLocationRange>,
    ivar_name: Option<RBSLocationRange>,
    visibility: Option<RBSLocationRange>,
}

fn build_attr_location(ctx: &mut MaterializeCtx<'_>, locs: AttrLocations) -> Result<Value, Error> {
    let loc = make_location(ctx, &locs.range)?;
    add_required_child(ctx, loc, "keyword", &locs.keyword)?;
    add_required_child(ctx, loc, "name", &locs.name)?;
    add_required_child(ctx, loc, "colon", &locs.colon)?;
    add_optional_child(ctx, loc, "kind", locs.kind.as_ref())?;
    add_optional_child(ctx, loc, "ivar", locs.ivar.as_ref())?;
    add_optional_child(ctx, loc, "ivar_name", locs.ivar_name.as_ref())?;
    add_optional_child(ctx, loc, "visibility", locs.visibility.as_ref())?;
    Ok(loc)
}

fn attr_accessor(
    ctx: &mut MaterializeCtx<'_>,
    node: &AttrAccessorNode<'_>,
) -> Result<Value, Error> {
    let loc = build_attr_location(
        ctx,
        AttrLocations {
            range: node.location(),
            keyword: node.keyword_location(),
            name: node.name_location(),
            colon: node.colon_location(),
            kind: node.kind_location(),
            ivar: node.ivar_location(),
            ivar_name: node.ivar_name_location(),
            visibility: node.visibility_location(),
        },
    )?;
    let name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    let ty = materialize_type(ctx, &node.type_())?;
    let ivar_name = attr_ivar_name(ctx, node.ivar_name(), node.ivar_name_location());
    let kind = attribute_kind(ctx, node.kind());
    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    let visibility = attribute_visibility(ctx, node.visibility());
    Ok(ctx
        .classes
        .members_attr_accessor
        .new_instance((kwargs!(
            "name" => name,
            "type" => ty,
            "ivar_name" => ivar_name,
            "kind" => kind,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment,
            "visibility" => visibility
        ),))?
        .as_value())
}

fn attr_reader(ctx: &mut MaterializeCtx<'_>, node: &AttrReaderNode<'_>) -> Result<Value, Error> {
    let loc = build_attr_location(
        ctx,
        AttrLocations {
            range: node.location(),
            keyword: node.keyword_location(),
            name: node.name_location(),
            colon: node.colon_location(),
            kind: node.kind_location(),
            ivar: node.ivar_location(),
            ivar_name: node.ivar_name_location(),
            visibility: node.visibility_location(),
        },
    )?;
    let name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    let ty = materialize_type(ctx, &node.type_())?;
    let ivar_name = attr_ivar_name(ctx, node.ivar_name(), node.ivar_name_location());
    let kind = attribute_kind(ctx, node.kind());
    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    let visibility = attribute_visibility(ctx, node.visibility());
    Ok(ctx
        .classes
        .members_attr_reader
        .new_instance((kwargs!(
            "name" => name,
            "type" => ty,
            "ivar_name" => ivar_name,
            "kind" => kind,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment,
            "visibility" => visibility
        ),))?
        .as_value())
}

fn attr_writer(ctx: &mut MaterializeCtx<'_>, node: &AttrWriterNode<'_>) -> Result<Value, Error> {
    let loc = build_attr_location(
        ctx,
        AttrLocations {
            range: node.location(),
            keyword: node.keyword_location(),
            name: node.name_location(),
            colon: node.colon_location(),
            kind: node.kind_location(),
            ivar: node.ivar_location(),
            ivar_name: node.ivar_name_location(),
            visibility: node.visibility_location(),
        },
    )?;
    let name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    let ty = materialize_type(ctx, &node.type_())?;
    let ivar_name = attr_ivar_name(ctx, node.ivar_name(), node.ivar_name_location());
    let kind = attribute_kind(ctx, node.kind());
    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    let visibility = attribute_visibility(ctx, node.visibility());
    Ok(ctx
        .classes
        .members_attr_writer
        .new_instance((kwargs!(
            "name" => name,
            "type" => ty,
            "ivar_name" => ivar_name,
            "kind" => kind,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment,
            "visibility" => visibility
        ),))?
        .as_value())
}

fn build_var_location(
    ctx: &mut MaterializeCtx<'_>,
    range: RBSLocationRange,
    name: RBSLocationRange,
    colon: RBSLocationRange,
    kind: Option<RBSLocationRange>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &range)?;
    add_required_child(ctx, loc, "name", &name)?;
    add_required_child(ctx, loc, "colon", &colon)?;
    add_optional_child(ctx, loc, "kind", kind.as_ref())?;
    Ok(loc)
}

fn instance_variable(
    ctx: &mut MaterializeCtx<'_>,
    node: &InstanceVariableNode<'_>,
) -> Result<Value, Error> {
    let loc = build_var_location(
        ctx,
        node.location(),
        node.name_location(),
        node.colon_location(),
        node.kind_location(),
    )?;
    let name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    let ty = materialize_type(ctx, &node.type_())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .members_instance_variable
        .new_instance((kwargs!(
            "name" => name,
            "type" => ty,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn class_instance_variable(
    ctx: &mut MaterializeCtx<'_>,
    node: &ClassInstanceVariableNode<'_>,
) -> Result<Value, Error> {
    let loc = build_var_location(
        ctx,
        node.location(),
        node.name_location(),
        node.colon_location(),
        node.kind_location(),
    )?;
    let name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    let ty = materialize_type(ctx, &node.type_())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .members_class_instance_variable
        .new_instance((kwargs!(
            "name" => name,
            "type" => ty,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn class_variable(
    ctx: &mut MaterializeCtx<'_>,
    node: &ClassVariableNode<'_>,
) -> Result<Value, Error> {
    let loc = build_var_location(
        ctx,
        node.location(),
        node.name_location(),
        node.colon_location(),
        node.kind_location(),
    )?;
    let name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    let ty = materialize_type(ctx, &node.type_())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .members_class_variable
        .new_instance((kwargs!(
            "name" => name,
            "type" => ty,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn build_mixin_location(
    ctx: &mut MaterializeCtx<'_>,
    range: RBSLocationRange,
    name: RBSLocationRange,
    keyword: RBSLocationRange,
    args: Option<RBSLocationRange>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &range)?;
    add_required_child(ctx, loc, "name", &name)?;
    add_required_child(ctx, loc, "keyword", &keyword)?;
    add_optional_child(ctx, loc, "args", args.as_ref())?;
    Ok(loc)
}

fn include_member(ctx: &mut MaterializeCtx<'_>, node: &IncludeNode<'_>) -> Result<Value, Error> {
    let loc = build_mixin_location(
        ctx,
        node.location(),
        node.name_location(),
        node.keyword_location(),
        node.args_location(),
    )?;
    let raw =
        find_type_name_node(ctx.interner, &node.name()).expect("mixin name pre-interned by insert");
    let name = materialize_resolved_type_name(ctx, raw)?;
    let args = build_args_array(ctx, node.args())?;
    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .members_include
        .new_instance((kwargs!(
            "name" => name,
            "args" => args,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn extend_member(ctx: &mut MaterializeCtx<'_>, node: &ExtendNode<'_>) -> Result<Value, Error> {
    let loc = build_mixin_location(
        ctx,
        node.location(),
        node.name_location(),
        node.keyword_location(),
        node.args_location(),
    )?;
    let raw =
        find_type_name_node(ctx.interner, &node.name()).expect("mixin name pre-interned by insert");
    let name = materialize_resolved_type_name(ctx, raw)?;
    let args = build_args_array(ctx, node.args())?;
    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .members_extend
        .new_instance((kwargs!(
            "name" => name,
            "args" => args,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn prepend_member(ctx: &mut MaterializeCtx<'_>, node: &PrependNode<'_>) -> Result<Value, Error> {
    let loc = build_mixin_location(
        ctx,
        node.location(),
        node.name_location(),
        node.keyword_location(),
        node.args_location(),
    )?;
    let raw =
        find_type_name_node(ctx.interner, &node.name()).expect("mixin name pre-interned by insert");
    let name = materialize_resolved_type_name(ctx, raw)?;
    let args = build_args_array(ctx, node.args())?;
    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .members_prepend
        .new_instance((kwargs!(
            "name" => name,
            "args" => args,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn alias_member(ctx: &mut MaterializeCtx<'_>, node: &AliasNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;
    add_required_child(ctx, loc, "new_name", &node.new_name_location())?;
    add_required_child(ctx, loc, "old_name", &node.old_name_location())?;
    add_optional_child(ctx, loc, "new_kind", node.new_kind_location().as_ref())?;
    add_optional_child(ctx, loc, "old_kind", node.old_kind_location().as_ref())?;

    let new_name = ctx.ruby.to_symbol(node.new_name().as_str()).as_value();
    let old_name = ctx.ruby.to_symbol(node.old_name().as_str()).as_value();
    let kind = alias_kind(ctx, node.kind());
    let annotations = build_annotations(ctx, node.annotations())?;
    let comment = build_comment(ctx, node.comment())?;
    Ok(ctx
        .classes
        .members_alias
        .new_instance((kwargs!(
            "new_name" => new_name,
            "old_name" => old_name,
            "kind" => kind,
            "annotations" => annotations,
            "location" => loc,
            "comment" => comment
        ),))?
        .as_value())
}

fn public_member(ctx: &mut MaterializeCtx<'_>, node: &PublicNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    Ok(ctx
        .classes
        .members_public
        .new_instance((kwargs!("location" => loc),))?
        .as_value())
}

fn private_member(ctx: &mut MaterializeCtx<'_>, node: &PrivateNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    Ok(ctx
        .classes
        .members_private
        .new_instance((kwargs!("location" => loc),))?
        .as_value())
}
