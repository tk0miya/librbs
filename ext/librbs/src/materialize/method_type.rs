//! M3g: build `RBS::MethodType` (and the optional `RBS::Types::Block`
//! it can carry) from `ruby_rbs::node::MethodTypeNode`.
//!
//! `materialize_block` lives here rather than in `type_.rs` because
//! `MethodType#block` and `ProcType#block` both want it, and the proc
//! type's `block_type` builder already calls into this helper for the
//! second consumer.

use magnus::{Error, Value, kwargs, prelude::*, value::ReprValue};

use ruby_rbs::node::{BlockTypeNode, MethodTypeNode};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{add_optional_child, add_required_child, make_location};
use crate::materialize::type_::materialize_type;
use crate::materialize::type_param::materialize_type_params;

/// Build `RBS::MethodType.new(type_params:, type:, block:, location:)`.
/// Mirrors `ast_translation.c::RBS_METHOD_TYPE` (vendor):
///
/// - location carries `type` (required) and `type_params` (optional)
///   sub-locations,
/// - `type_params` are materialized through M3f's helper and then run
///   through `RBS::AST::TypeParam.resolve_variables`,
/// - `type` is required to be a `Function` / `UntypedFunction` from the
///   parser's grammar; we route it through `materialize_type` and let
///   the dispatch panic if a non-function shape ever arrives,
/// - `block` is an optional `RBS::Types::Block`.
pub fn materialize_method_type(
    ctx: &mut MaterializeCtx<'_>,
    node: &MethodTypeNode<'_>,
) -> Result<Value, Error> {
    let _t = crate::materialize::phase_timer::PhaseTimer::new(
        crate::materialize::phase_timer::Phase::MethodType,
    );
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "type", &node.type_location())?;
    add_optional_child(
        ctx,
        loc,
        "type_params",
        node.type_params_location().as_ref(),
    )?;

    let type_params = materialize_type_params(ctx, node.type_params())?;

    let func_node = node.type_();
    let func = materialize_type(ctx, &func_node)?;

    let block: Value = match node.block() {
        Some(b) => materialize_block(ctx, &b)?,
        None => ctx.ruby.qnil().as_value(),
    };

    Ok(ctx
        .classes
        .method_type
        .new_instance((kwargs!(
            "type_params" => type_params,
            "type" => func,
            "block" => block,
            "location" => loc
        ),))?
        .as_value())
}

/// Build `RBS::Types::Block.new(type:, required:, self_type:)`. Used by
/// both `materialize_method_type` (here) and the proc-type builder in
/// `type_::block_type` (which wraps this).
pub fn materialize_block(
    ctx: &mut MaterializeCtx<'_>,
    node: &BlockTypeNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    let func = materialize_type(ctx, &node.type_())?;
    let self_type: Value = match node.self_type() {
        Some(t) => materialize_type(ctx, &t)?,
        None => ctx.ruby.qnil().as_value(),
    };
    Ok(ctx
        .classes
        .types_block
        .new_instance((kwargs!(
            "type" => func,
            "required" => node.required(),
            "self_type" => self_type,
            "location" => loc
        ),))?
        .as_value())
}
