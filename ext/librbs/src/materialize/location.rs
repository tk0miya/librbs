//! Helpers for constructing `RBS::Location` and its sub-locations from
//! parser-emitted [`RBSLocationRange`]s.
//!
//! Character offsets come straight from the parser via
//! `RBSLocationRange::start_char` / `end_char` (the local fork of
//! ruby-rbs adds these accessors); no byte → char conversion happens
//! on the Rust side. The prerequisite parser rewrite is what closes
//! the M2 followup "Byte ↔ character offset bridge for `RBS::Location`".

use magnus::{Error, IntoValue, Value, prelude::*, value::ReprValue};

use ruby_rbs::node::RBSLocationRange;

use crate::materialize::MaterializeCtx;
use crate::materialize::phase_timer::{Phase, PhaseTimer};

/// `RBS::Location.new(buffer, start_char, end_char)` for the current
/// source. Reads the active buffer from [`MaterializeCtx::buffer`].
pub fn make_location(ctx: &MaterializeCtx<'_>, range: &RBSLocationRange) -> Result<Value, Error> {
    let _t = PhaseTimer::new(Phase::Location);
    let buffer = ctx.buffer();
    let start = range.start_char();
    let end = range.end_char();
    Ok(ctx
        .classes
        .location
        .new_instance((buffer, start, end))?
        .as_value())
}

/// Append a required sub-location at `name` for `range`. Mirrors
/// `RBS::Location#add_required_child(name, range)` from
/// `vendor/rbs/lib/rbs/location_aux.rb`.
pub fn add_required_child(
    ctx: &MaterializeCtx<'_>,
    loc: Value,
    name: &str,
    range: &RBSLocationRange,
) -> Result<(), Error> {
    let sym = ctx.ruby.to_symbol(name);
    let r = ctx
        .ruby
        .range_new(range.start_char(), range.end_char(), false)?;
    let _: Value = loc.funcall("add_required_child", (sym, r))?;
    Ok(())
}

/// Append an optional sub-location at `name`. When `range` is `None`
/// the upstream method is called with `nil`, mirroring
/// `RBS::Location#add_optional_child(name, nil)`.
pub fn add_optional_child(
    ctx: &MaterializeCtx<'_>,
    loc: Value,
    name: &str,
    range: Option<&RBSLocationRange>,
) -> Result<(), Error> {
    let sym = ctx.ruby.to_symbol(name);
    let r: Value = match range {
        Some(r) => ctx
            .ruby
            .range_new(r.start_char(), r.end_char(), false)?
            .into_value_with(ctx.ruby),
        None => ctx.ruby.qnil().as_value(),
    };
    let _: Value = loc.funcall("add_optional_child", (sym, r))?;
    Ok(())
}
