//! M3k: build `RBS::Source::RBS` instances for each
//! `librbs_core::source::Source`.
//!
//! `Source::Ruby` is loader-only via M5's `add_source` patch — the
//! current loader emits no Ruby sources, so the `Ruby` branch is left
//! `unreachable!()` until that lands.

use magnus::{Error, RArray, Value, prelude::*, value::ReprValue};

use librbs_core::Source;

use crate::materialize::MaterializeCtx;

/// Build `RBS::Source::RBS.new(buffer, directives, declarations)` for
/// the given Rust source. `buffer` is reused from `MaterializeCtx`'s
/// per-source cache so every `Location` in the materialized tree
/// shares the same Ruby `RBS::Buffer` value as `Source::RBS#buffer`.
pub fn build_rbs_source(
    ctx: &mut MaterializeCtx<'_>,
    _src: &Source,
    directives: RArray,
    declarations: RArray,
) -> Result<Value, Error> {
    let buffer = ctx.buffer()?;
    Ok(ctx
        .classes
        .source_rbs
        .new_instance((buffer, directives, declarations))?
        .as_value())
}
