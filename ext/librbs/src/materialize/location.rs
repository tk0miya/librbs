//! Helpers for constructing `RBS::Location` and its sub-locations from
//! parser-emitted [`RBSLocationRange`]s.
//!
//! Character offsets come straight from the parser via
//! `RBSLocationRange::start_char` / `end_char` (the local fork of
//! ruby-rbs adds these accessors); no byte → char conversion happens
//! on the Rust side.
//!
//! `RBS::Location` construction is **deferred entirely**. Instead of
//! allocating a real `RBS::Location` per node and dispatching 5–7
//! `add_required_child` / `add_optional_child` calls, the native side
//! returns a Ruby `Array` "spec" that represents the location:
//!
//!   [buffer, start, end]                  (no children)
//!   [buffer, start, end, children_flat]   (with children)
//!
//! `children_flat` is a flat Array of 4-tuples `(kind_sym, name_sym,
//! start_or_nil, end_or_nil)`. The spec is passed verbatim into the
//! upstream class initializer as the `location:` kwarg, where it lands
//! in `@location`. `Librbs::Patches::LazyLocation`'s prepended
//! `location` reader detects the Array form on first access and
//! realises the real `RBS::Location` (and its children) lazily.
//!
//! For the load + resolve + `add_source` path no caller reads
//! `.location`, so the realiser never runs — saving ~60K
//! `RBS::Location.new` allocations and ~300K `_add_*_child` C-side
//! calls per materialise.

use magnus::{Error, RArray, TryConvert, Value, value::ReprValue};

use ruby_rbs::node::RBSLocationRange;

use crate::materialize::MaterializeCtx;
use crate::materialize::phase_timer::{Phase, PhaseTimer};

/// Build a deferred-Location spec Array `[buffer, start, end]` for
/// `range`. The returned `Value` is a Ruby `Array` (not an
/// `RBS::Location`); pass it through to the upstream class
/// initializer as the `location:` kwarg. The lazy reader prepended on
/// each RBS class converts it on first access (see
/// `lib/librbs/patches/location.rb`).
pub fn make_location(ctx: &MaterializeCtx<'_>, range: &RBSLocationRange) -> Result<Value, Error> {
    let _t = PhaseTimer::new(Phase::Location);
    let arr = ctx.ruby.ary_new_capa(3);
    arr.push(ctx.buffer())?;
    arr.push(range.start_char())?;
    arr.push(range.end_char())?;
    Ok(arr.as_value())
}

/// Lazily attach (or fetch) the children sub-array on a deferred
/// location spec. The first time a child is added, the spec grows
/// from `[buffer, start, end]` to `[buffer, start, end, children]`.
fn ensure_children(ctx: &MaterializeCtx<'_>, spec: RArray) -> Result<RArray, Error> {
    if spec.len() < 4 {
        let c = ctx.ruby.ary_new();
        spec.push(c.as_value())?;
        Ok(c)
    } else {
        RArray::try_convert(spec.entry::<Value>(3)?)
    }
}

/// Push a deferred required-child row onto `loc`. Mirrors
/// `RBS::Location#add_required_child(name, range)` semantically; the
/// row is realised by the lazy reader the first time someone reads
/// `.location`.
///
/// Intentionally untimed: ~1 M invocations per materialise would
/// dwarf real cost with `PhaseTimer` overhead.
pub fn add_required_child(
    ctx: &MaterializeCtx<'_>,
    loc: Value,
    name: &str,
    range: &RBSLocationRange,
) -> Result<(), Error> {
    let spec = RArray::try_convert(loc)?;
    let children = ensure_children(ctx, spec)?;
    children.push(ctx.ruby.to_symbol("required"))?;
    children.push(ctx.ruby.to_symbol(name))?;
    children.push(range.start_char())?;
    children.push(range.end_char())?;
    Ok(())
}

/// Push a deferred optional-child row onto `loc`. `None` materialises
/// as `:optional_absent` (mirroring `_add_optional_no_child`); `Some`
/// materialises as `:optional_present` with the range bounds. Both
/// variants emit 4 elements so the realiser can `each_slice(4)`
/// uniformly.
pub fn add_optional_child(
    ctx: &MaterializeCtx<'_>,
    loc: Value,
    name: &str,
    range: Option<&RBSLocationRange>,
) -> Result<(), Error> {
    let spec = RArray::try_convert(loc)?;
    let children = ensure_children(ctx, spec)?;
    let nil = ctx.ruby.qnil().as_value();
    match range {
        Some(r) => {
            children.push(ctx.ruby.to_symbol("optional_present"))?;
            children.push(ctx.ruby.to_symbol(name))?;
            children.push(r.start_char())?;
            children.push(r.end_char())?;
        }
        None => {
            children.push(ctx.ruby.to_symbol("optional_absent"))?;
            children.push(ctx.ruby.to_symbol(name))?;
            children.push(nil)?;
            children.push(nil)?;
        }
    }
    Ok(())
}
