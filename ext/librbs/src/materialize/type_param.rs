//! Build `RBS::AST::TypeParam` from a `TypeParamNode`. Used by both
//! method-type `type_params` and decl-level `type_params`.

use magnus::{Error, RArray, Value, kwargs, prelude::*, value::ReprValue};

use ruby_rbs::node::{Node, TypeParamNode, TypeParamVariance};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{
    add_optional_child, add_required_child, alloc_children, make_location,
};
use crate::materialize::type_::materialize_type;

pub fn materialize_type_param(
    ctx: &mut MaterializeCtx<'_>,
    node: &TypeParamNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    alloc_children(ctx, loc, 6);
    add_required_child(ctx, loc, "name", node.name_location())?;
    add_optional_child(ctx, loc, "variance", node.variance_location())?;
    add_optional_child(ctx, loc, "unchecked", node.unchecked_location())?;
    add_optional_child(ctx, loc, "upper_bound", node.upper_bound_location())?;
    add_optional_child(ctx, loc, "lower_bound", node.lower_bound_location())?;
    add_optional_child(ctx, loc, "default", node.default_location())?;

    let name = ctx.symbol_for_str(node.name().as_str());
    let variance = match node.variance() {
        TypeParamVariance::Invariant => ctx.common.invariant,
        TypeParamVariance::Covariant => ctx.common.covariant,
        TypeParamVariance::Contravariant => ctx.common.contravariant,
    };
    let upper_bound: Value = match node.upper_bound() {
        Some(n) => materialize_type(ctx, &n)?,
        None => ctx.ruby.qnil().as_value(),
    };
    let lower_bound: Value = match node.lower_bound() {
        Some(n) => materialize_type(ctx, &n)?,
        None => ctx.ruby.qnil().as_value(),
    };
    let default_type: Value = match node.default_type() {
        Some(n) => materialize_type(ctx, &n)?,
        None => ctx.ruby.qnil().as_value(),
    };

    Ok(ctx
        .classes
        .type_param
        .new_instance((kwargs!(
            "name" => name,
            "variance" => variance,
            "upper_bound" => upper_bound,
            "lower_bound" => lower_bound,
            "default_type" => default_type,
            "unchecked" => node.unchecked(),
            "location" => loc
        ),))?
        .as_value())
}

/// Materialize every `TypeParam` node in `list`, then run the upstream
/// `RBS::AST::TypeParam.resolve_variables(arr)` post-pass. Both
/// declaration-level (`class Foo[X < _Each[Y], Y]`) and method-type
/// (`def foo: [X] -> X`) call sites need the resolve_variables step,
/// so the helper lives here rather than at the call sites.
pub fn materialize_type_params(
    ctx: &mut MaterializeCtx<'_>,
    list: ruby_rbs::node::NodeList<'_>,
) -> Result<RArray, Error> {
    let arr = ctx.ruby.ary_new_capa(list.len());
    for p in list.iter() {
        let Node::TypeParam(tp) = &p else {
            unreachable!("type_params list must hold TypeParam nodes only");
        };
        let tp: &TypeParamNode<'_> = tp;
        arr.push(materialize_type_param(ctx, tp)?)?;
    }
    let _: Value = ctx
        .classes
        .type_param
        .funcall("resolve_variables", (arr,))?;
    Ok(arr)
}
