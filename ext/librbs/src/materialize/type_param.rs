//! M3f: build `RBS::AST::TypeParam` from a `TypeParamNode`. Used by
//! M3g (method-type `type_params`) and M3h (decl-level `type_params`).

use magnus::{Error, Value, kwargs, prelude::*, value::ReprValue};

use ruby_rbs::node::{TypeParamNode, TypeParamVariance};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{add_optional_child, add_required_child, make_location};
use crate::materialize::type_::materialize_type;

pub fn materialize_type_param(
    ctx: &mut MaterializeCtx<'_>,
    node: &TypeParamNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "name", &node.name_location())?;
    add_optional_child(ctx, loc, "variance", node.variance_location().as_ref())?;
    add_optional_child(ctx, loc, "unchecked", node.unchecked_location().as_ref())?;
    add_optional_child(
        ctx,
        loc,
        "upper_bound",
        node.upper_bound_location().as_ref(),
    )?;
    add_optional_child(
        ctx,
        loc,
        "lower_bound",
        node.lower_bound_location().as_ref(),
    )?;
    add_optional_child(ctx, loc, "default", node.default_location().as_ref())?;

    let name = ctx.ruby.to_symbol(node.name().as_str());
    let variance = match node.variance() {
        TypeParamVariance::Invariant => ctx.ruby.to_symbol("invariant"),
        TypeParamVariance::Covariant => ctx.ruby.to_symbol("covariant"),
        TypeParamVariance::Contravariant => ctx.ruby.to_symbol("contravariant"),
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
