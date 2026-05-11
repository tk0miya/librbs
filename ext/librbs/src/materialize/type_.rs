//! M3f: build `RBS::Types::*` instances from `ruby_rbs::node` type nodes.
//!
//! [`materialize_type`] is the dispatch entry point. Every arm mirrors
//! one variant in `crates/librbs-core/src/resolver/driver.rs::walk_type`
//! so the two walks stay in lockstep — `_ =>` fallthroughs are
//! forbidden so the compiler flags any future drift.
//!
//! Type-name occurrences (`ClassInstance`, `Interface`, `Alias`,
//! `ClassSingleton`) consult the per-decl resolution cursor on
//! [`MaterializeCtx`] via [`materialize_resolved_type_name`] in the
//! same pre-order the driver pushes them.

use magnus::{Error, IntoValue, RArray, Value, kwargs, prelude::*, value::ReprValue};

use librbs_core::env::insert::find_type_name_node;
use ruby_rbs::node::{
    AliasTypeNode, AnyTypeNode, ClassInstanceTypeNode, ClassSingletonTypeNode, FunctionTypeNode,
    InterfaceTypeNode, IntersectionTypeNode, LiteralTypeNode, Node, OptionalTypeNode, ProcTypeNode,
    RecordTypeNode, TupleTypeNode, UnionTypeNode, UntypedFunctionTypeNode, VariableTypeNode,
};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{
    add_optional_child, add_required_child, alloc_children, make_location,
};
use crate::materialize::method_type::materialize_block;
use crate::materialize::type_name::materialize_resolved_type_name;

/// Dispatch a `Node` representing an `RBS::Types::*` variant into the
/// matching builder. Panics with `unreachable!` on any non-type
/// `Node` variant — the exhaustive match arms cover every variant
/// listed in `walk_type`.
pub fn materialize_type(ctx: &mut MaterializeCtx<'_>, node: &Node<'_>) -> Result<Value, Error> {
    match node {
        Node::BoolType(_) => bases_only(ctx, node, ctx.classes.types_bases_bool),
        Node::VoidType(_) => bases_only(ctx, node, ctx.classes.types_bases_void),
        Node::NilType(_) => bases_only(ctx, node, ctx.classes.types_bases_nil),
        Node::TopType(_) => bases_only(ctx, node, ctx.classes.types_bases_top),
        Node::BottomType(_) => bases_only(ctx, node, ctx.classes.types_bases_bottom),
        Node::SelfType(_) => bases_only(ctx, node, ctx.classes.types_bases_self),
        Node::InstanceType(_) => bases_only(ctx, node, ctx.classes.types_bases_instance),
        Node::ClassType(_) => bases_only(ctx, node, ctx.classes.types_bases_class),
        Node::AnyType(t) => any_type(ctx, t),
        Node::VariableType(t) => variable_type(ctx, t),
        Node::LiteralType(t) => literal_type(ctx, t),
        Node::ClassInstanceType(t) => class_instance_type(ctx, t),
        Node::InterfaceType(t) => interface_type(ctx, t),
        Node::AliasType(t) => alias_type(ctx, t),
        Node::ClassSingletonType(t) => class_singleton_type(ctx, t),
        Node::TupleType(t) => tuple_type(ctx, t),
        Node::UnionType(t) => union_type(ctx, t),
        Node::IntersectionType(t) => intersection_type(ctx, t),
        Node::RecordType(t) => record_type(ctx, t),
        Node::OptionalType(t) => optional_type(ctx, t),
        Node::ProcType(t) => proc_type(ctx, t),
        Node::FunctionType(t) => function_type(ctx, t),
        Node::UntypedFunctionType(t) => untyped_function_type(ctx, t),
        Node::BlockType(t) => materialize_block(ctx, t),
        // Listing every other AST variant exhaustively here would force
        // a recompile each time the parser adds a new node. Instead we
        // panic — this branch is unreachable as long as
        // `materialize_type` is only ever called from the type walk
        // (which mirrors `walk_type`); a non-type node arriving here
        // signals a parity bug between the two walks.
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
        | Node::Alias(_)
        | Node::AttrAccessor(_)
        | Node::AttrReader(_)
        | Node::AttrWriter(_)
        | Node::ClassVariable(_)
        | Node::ClassInstanceVariable(_)
        | Node::Extend(_)
        | Node::Include(_)
        | Node::Prepend(_)
        | Node::InstanceVariable(_)
        | Node::MethodDefinition(_)
        | Node::MethodDefinitionOverload(_)
        | Node::Private(_)
        | Node::Public(_)
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
            unreachable!("materialize_type called on non-type node variant; walk parity bug")
        }
    }
}

fn bases_only(
    ctx: &mut MaterializeCtx<'_>,
    node: &Node<'_>,
    class: magnus::RClass,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    Ok(class
        .new_instance((kwargs!("location" => loc),))?
        .as_value())
}

fn any_type(ctx: &mut MaterializeCtx<'_>, node: &AnyTypeNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    Ok(ctx
        .classes
        .types_bases_any
        .new_instance((kwargs!("location" => loc, "todo" => node.todo()),))?
        .as_value())
}

fn variable_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &VariableTypeNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let name = ctx.symbol_for_str(node.name().as_str());
    Ok(ctx
        .classes
        .types_variable
        .new_instance((kwargs!("name" => name, "location" => loc),))?
        .as_value())
}

fn literal_type(ctx: &mut MaterializeCtx<'_>, node: &LiteralTypeNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let literal: Value = match node.literal() {
        // Mirror upstream `ast_translation.c::RBS_AST_INTEGER`: take the
        // string representation as written and call `String#to_i`.
        Node::Integer(int) => {
            let s = int.string_representation().as_str().to_string();
            let str_v: Value = s.into_value_with(ctx.ruby);
            str_v.funcall("to_i", ())?
        }
        // `RBS_AST_STRING`: the parser already unquoted the source token.
        Node::String(s) => {
            let raw = s.string().as_str().to_string();
            raw.into_value_with(ctx.ruby)
        }
        Node::Symbol(sym) => ctx.symbol_for_str(sym.as_str()),
        Node::Bool(b) => {
            // BoolNode exposes `value()` for the boolean payload.
            if b.value() {
                ctx.ruby.qtrue().as_value()
            } else {
                ctx.ruby.qfalse().as_value()
            }
        }
        // Other literal-child types are not reachable from RBS source.
        _ => unreachable!("RBS::Types::Literal child must be Integer/String/Symbol/Bool"),
    };
    Ok(ctx
        .classes
        .types_literal
        .new_instance((kwargs!("literal" => literal, "location" => loc),))?
        .as_value())
}

fn name_args_location(
    ctx: &mut MaterializeCtx<'_>,
    range: ruby_rbs::node::RBSLocationRange,
    name_range: ruby_rbs::node::RBSLocationRange,
    args_range: Option<ruby_rbs::node::RBSLocationRange>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &range)?;
    alloc_children(ctx, loc, 2);
    add_required_child(ctx, loc, "name", name_range)?;
    add_optional_child(ctx, loc, "args", args_range)?;
    Ok(loc)
}

fn build_args_array(
    ctx: &mut MaterializeCtx<'_>,
    args: ruby_rbs::node::NodeList<'_>,
) -> Result<RArray, Error> {
    let arr = ctx.ruby.ary_new_capa(args.len());
    for a in args.iter() {
        arr.push(materialize_type(ctx, &a)?)?;
    }
    Ok(arr)
}

fn class_instance_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &ClassInstanceTypeNode<'_>,
) -> Result<Value, Error> {
    let loc = name_args_location(
        ctx,
        node.location(),
        node.name_location(),
        node.args_location(),
    )?;
    let raw_name = find_type_name_node(ctx.interner, &node.name()).expect("name pre-interned");
    let name = materialize_resolved_type_name(ctx, raw_name)?;
    let args = build_args_array(ctx, node.args())?;
    Ok(ctx
        .classes
        .types_class_instance
        .new_instance((kwargs!("name" => name, "args" => args, "location" => loc),))?
        .as_value())
}

fn interface_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &InterfaceTypeNode<'_>,
) -> Result<Value, Error> {
    let loc = name_args_location(
        ctx,
        node.location(),
        node.name_location(),
        node.args_location(),
    )?;
    let raw_name = find_type_name_node(ctx.interner, &node.name()).expect("name pre-interned");
    let name = materialize_resolved_type_name(ctx, raw_name)?;
    let args = build_args_array(ctx, node.args())?;
    Ok(ctx
        .classes
        .types_interface
        .new_instance((kwargs!("name" => name, "args" => args, "location" => loc),))?
        .as_value())
}

fn alias_type(ctx: &mut MaterializeCtx<'_>, node: &AliasTypeNode<'_>) -> Result<Value, Error> {
    let loc = name_args_location(
        ctx,
        node.location(),
        node.name_location(),
        node.args_location(),
    )?;
    let raw_name = find_type_name_node(ctx.interner, &node.name()).expect("name pre-interned");
    let name = materialize_resolved_type_name(ctx, raw_name)?;
    let args = build_args_array(ctx, node.args())?;
    Ok(ctx
        .classes
        .types_alias
        .new_instance((kwargs!("name" => name, "args" => args, "location" => loc),))?
        .as_value())
}

fn class_singleton_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &ClassSingletonTypeNode<'_>,
) -> Result<Value, Error> {
    let loc = name_args_location(
        ctx,
        node.location(),
        node.name_location(),
        node.args_location(),
    )?;
    let raw_name = find_type_name_node(ctx.interner, &node.name()).expect("name pre-interned");
    let name = materialize_resolved_type_name(ctx, raw_name)?;
    // Upstream `ast_translation.c::RBS_TYPES_CLASS_SINGLETON` passes
    // `args:` even though `singleton(X)` syntax never carries generics
    // — keep the kwarg shape identical so canonical-dump compares
    // byte-for-byte.
    let args = build_args_array(ctx, node.args())?;
    Ok(ctx
        .classes
        .types_class_singleton
        .new_instance((kwargs!("name" => name, "args" => args, "location" => loc),))?
        .as_value())
}

fn types_array(
    ctx: &mut MaterializeCtx<'_>,
    types: ruby_rbs::node::NodeList<'_>,
) -> Result<RArray, Error> {
    let arr = ctx.ruby.ary_new_capa(types.len());
    for t in types.iter() {
        arr.push(materialize_type(ctx, &t)?)?;
    }
    Ok(arr)
}

fn tuple_type(ctx: &mut MaterializeCtx<'_>, node: &TupleTypeNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let types = types_array(ctx, node.types())?;
    Ok(ctx
        .classes
        .types_tuple
        .new_instance((kwargs!("types" => types, "location" => loc),))?
        .as_value())
}

fn union_type(ctx: &mut MaterializeCtx<'_>, node: &UnionTypeNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let types = types_array(ctx, node.types())?;
    Ok(ctx
        .classes
        .types_union
        .new_instance((kwargs!("types" => types, "location" => loc),))?
        .as_value())
}

fn intersection_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &IntersectionTypeNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let types = types_array(ctx, node.types())?;
    Ok(ctx
        .classes
        .types_intersection
        .new_instance((kwargs!("types" => types, "location" => loc),))?
        .as_value())
}

fn record_type(ctx: &mut MaterializeCtx<'_>, node: &RecordTypeNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let fields_hash = node.all_fields();
    let all_fields = ctx.ruby.hash_new_capa(fields_hash.len());
    for (key, value) in fields_hash.iter() {
        let key_sym = match &key {
            Node::Symbol(s) => ctx.symbol_for_str(s.as_str()),
            // RecordType key shapes are always Symbol per the C parser
            // (`vendor/rbs/ext/rbs_extension/ast_translation.c`).
            _ => unreachable!("RBS::Types::Record key must be Symbol"),
        };
        let Node::RecordFieldType(field) = &value else {
            unreachable!("RBS::Types::Record value must be RecordFieldType");
        };
        let ty = materialize_type(ctx, &field.type_())?;
        let pair = ctx.ruby.ary_new_capa(2);
        pair.push(ty)?;
        pair.push(field.required())?;
        all_fields.aset(key_sym, pair)?;
    }
    Ok(ctx
        .classes
        .types_record
        .new_instance((kwargs!("all_fields" => all_fields, "location" => loc),))?
        .as_value())
}

fn optional_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &OptionalTypeNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let inner = materialize_type(ctx, &node.type_())?;
    Ok(ctx
        .classes
        .types_optional
        .new_instance((kwargs!("type" => inner, "location" => loc),))?
        .as_value())
}

fn proc_type(ctx: &mut MaterializeCtx<'_>, node: &ProcTypeNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let func = materialize_type(ctx, &node.type_())?;
    let block = match node.block() {
        Some(b) => materialize_block(ctx, &b)?,
        None => ctx.ruby.qnil().as_value(),
    };
    let self_type = match node.self_type() {
        Some(t) => materialize_type(ctx, &t)?,
        None => ctx.ruby.qnil().as_value(),
    };
    Ok(ctx
        .classes
        .types_proc
        .new_instance((kwargs!(
            "type" => func,
            "block" => block,
            "self_type" => self_type,
            "location" => loc
        ),))?
        .as_value())
}

fn function_param(ctx: &mut MaterializeCtx<'_>, node: &Node<'_>) -> Result<Value, Error> {
    let Node::FunctionParam(p) = node else {
        unreachable!("function param list must hold FunctionParam nodes");
    };
    let loc = make_location(ctx, &p.location())?;
    add_optional_child(ctx, loc, "name", p.name_location())?;
    let ty = materialize_type(ctx, &p.type_())?;
    let name: Value = match p.name() {
        Some(sym) => ctx.symbol_for_str(sym.as_str()),
        None => ctx.ruby.qnil().as_value(),
    };
    Ok(ctx
        .classes
        .types_function_param
        .new_instance((kwargs!("type" => ty, "name" => name, "location" => loc),))?
        .as_value())
}

fn function_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &FunctionTypeNode<'_>,
) -> Result<Value, Error> {
    let req_pos_list = node.required_positionals();
    let required_positionals = ctx.ruby.ary_new_capa(req_pos_list.len());
    for p in req_pos_list.iter() {
        required_positionals.push(function_param(ctx, &p)?)?;
    }
    let opt_pos_list = node.optional_positionals();
    let optional_positionals = ctx.ruby.ary_new_capa(opt_pos_list.len());
    for p in opt_pos_list.iter() {
        optional_positionals.push(function_param(ctx, &p)?)?;
    }
    let rest_positionals: Value = match node.rest_positionals() {
        Some(p) => function_param(ctx, &p)?,
        None => ctx.ruby.qnil().as_value(),
    };
    let trail_pos_list = node.trailing_positionals();
    let trailing_positionals = ctx.ruby.ary_new_capa(trail_pos_list.len());
    for p in trail_pos_list.iter() {
        trailing_positionals.push(function_param(ctx, &p)?)?;
    }
    let req_kw_hash = node.required_keywords();
    let required_keywords = ctx.ruby.hash_new_capa(req_kw_hash.len());
    for (key, value) in req_kw_hash.iter() {
        let Node::Symbol(s) = &key else {
            unreachable!("required_keywords key must be Symbol");
        };
        required_keywords.aset(ctx.symbol_for_str(s.as_str()), function_param(ctx, &value)?)?;
    }
    let opt_kw_hash = node.optional_keywords();
    let optional_keywords = ctx.ruby.hash_new_capa(opt_kw_hash.len());
    for (key, value) in opt_kw_hash.iter() {
        let Node::Symbol(s) = &key else {
            unreachable!("optional_keywords key must be Symbol");
        };
        optional_keywords.aset(ctx.symbol_for_str(s.as_str()), function_param(ctx, &value)?)?;
    }
    let rest_keywords: Value = match node.rest_keywords() {
        Some(p) => function_param(ctx, &p)?,
        None => ctx.ruby.qnil().as_value(),
    };
    let return_type = materialize_type(ctx, &node.return_type())?;
    Ok(ctx
        .classes
        .types_function
        .new_instance((kwargs!(
            "required_positionals" => required_positionals,
            "optional_positionals" => optional_positionals,
            "rest_positionals" => rest_positionals,
            "trailing_positionals" => trailing_positionals,
            "required_keywords" => required_keywords,
            "optional_keywords" => optional_keywords,
            "rest_keywords" => rest_keywords,
            "return_type" => return_type
        ),))?
        .as_value())
}

fn untyped_function_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &UntypedFunctionTypeNode<'_>,
) -> Result<Value, Error> {
    let return_type = materialize_type(ctx, &node.return_type())?;
    Ok(ctx
        .classes
        .types_untyped_function
        .new_instance((kwargs!("return_type" => return_type),))?
        .as_value())
}
