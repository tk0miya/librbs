//! Helpers for constructing `RBS::Location` and its sub-locations from
//! parser-emitted [`RBSLocationRange`]s.
//!
//! Character offsets come straight from the parser via
//! `RBSLocationRange::start_char` / `end_char` (the local fork of
//! ruby-rbs adds these accessors); no byte → char conversion happens
//! on the Rust side.
//!
//! Sub-location construction is **deferred**. Instead of dispatching
//! `RBS::Location#add_required_child` / `add_optional_child` per child
//! at materialise time (each call: build a Range object + Ruby method
//! dispatch into a C-defined `_add_*_child`), we push a row onto the
//! Location's `@__librbs_pending_children` Array and let
//! `Librbs::Patches::Location` realise it the first time any reader
//! method (`[]`, `each_required_key`, `_required_keys`, `inspect`, …)
//! runs. For the load + resolve + `add_source` path no such reader
//! runs, so the children never get built — saving ~5–7 funcalls per
//! Location across ~60K Locations per materialise.
//!
//! Row encoding (matches `lib/librbs/patches/location.rb`):
//!   [:required,         <name Symbol>, <Integer start>, <Integer end>]
//!   [:optional_present, <name Symbol>, <Integer start>, <Integer end>]
//!   [:optional_absent,  <name Symbol>]

use magnus::{Error, RArray, RTypedData, TryConvert, Value, prelude::*, value::ReprValue};

use ruby_rbs::node::RBSLocationRange;

use crate::materialize::MaterializeCtx;
use crate::materialize::phase_timer::{Phase, PhaseTimer};

const PENDING_IVAR: &str = "@__librbs_pending_children";

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

/// Get-or-create the deferred child queue stashed on `loc`. `RBS::Location`
/// is `T_DATA` (C-extension `TypedData`); `RTypedData` implements
/// magnus's `Object` trait so we can call `ivar_get` / `ivar_set`
/// directly into Ruby's `rb_ivar_get` / `rb_ivar_set` (no Ruby-method
/// dispatch).
fn pending_children(ctx: &MaterializeCtx<'_>, loc: Value) -> Result<RArray, Error> {
    let typed = RTypedData::from_value(loc).ok_or_else(|| {
        magnus::Error::new(ctx.ruby.exception_type_error(), "Location is not TypedData")
    })?;
    let v: Value = typed.ivar_get(PENDING_IVAR)?;
    if v.is_nil() {
        let arr = ctx.ruby.ary_new();
        typed.ivar_set(PENDING_IVAR, arr.as_value())?;
        Ok(arr)
    } else {
        RArray::try_convert(v)
    }
}

/// Push a deferred required-child row onto `loc`. Mirrors
/// `RBS::Location#add_required_child(name, range)` semantically, but
/// without the per-call funcall (see module docs).
///
/// Intentionally untimed: 1.5M+ invocations per materialise would
/// turn the ~100 ns `PhaseTimer` into a multi-hundred-ms phantom
/// charge. The work is included in the parent (member / declaration
/// / method_type) phase's self-time instead.
pub fn add_required_child(
    ctx: &MaterializeCtx<'_>,
    loc: Value,
    name: &str,
    range: &RBSLocationRange,
) -> Result<(), Error> {
    let pending = pending_children(ctx, loc)?;
    let row = ctx.ruby.ary_new_capa(4);
    row.push(ctx.ruby.to_symbol("required"))?;
    row.push(ctx.ruby.to_symbol(name))?;
    row.push(range.start_char())?;
    row.push(range.end_char())?;
    pending.push(row)?;
    Ok(())
}

/// Push a deferred optional-child row onto `loc`. `None` materialises
/// as `:optional_absent` (mirroring `_add_optional_no_child`); `Some`
/// materialises as `:optional_present` with the range bounds.
pub fn add_optional_child(
    ctx: &MaterializeCtx<'_>,
    loc: Value,
    name: &str,
    range: Option<&RBSLocationRange>,
) -> Result<(), Error> {
    let pending = pending_children(ctx, loc)?;
    let row = match range {
        Some(r) => {
            let row = ctx.ruby.ary_new_capa(4);
            row.push(ctx.ruby.to_symbol("optional_present"))?;
            row.push(ctx.ruby.to_symbol(name))?;
            row.push(r.start_char())?;
            row.push(r.end_char())?;
            row
        }
        None => {
            let row = ctx.ruby.ary_new_capa(2);
            row.push(ctx.ruby.to_symbol("optional_absent"))?;
            row.push(ctx.ruby.to_symbol(name))?;
            row
        }
    };
    pending.push(row)?;
    Ok(())
}
