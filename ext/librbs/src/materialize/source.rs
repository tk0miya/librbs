//! M3k: build `RBS::Source::RBS` (and stub `Source::Ruby`) instances.
//!
//! Each `librbs_core::Source` becomes one `Source::RBS` whose
//! `buffer` / `directives` / `declarations` are reachable from
//! `RBS::Environment#sources`. The Ruby decl `Value`s threaded into
//! `declarations` here share Ruby object identity with the values
//! sitting inside the `*_decls` Entries — that is, the upstream
//! `source.declarations[i].equal?(class_decls[name].decls[k].decl)`
//! invariant holds within one env (cross-env identity is documented in
//! the M3k doc as out-of-scope).
//!
//! The Ruby-source dispatch arm is reachable only from M5's
//! `add_source` path. Loader-only flows (M3) emit no Ruby sources, so
//! the branch panics here and is documented in the M5 task doc.

use magnus::{Error, RArray, Value, prelude::*, value::ReprValue};

use librbs_core::Source;

use crate::materialize::MaterializeCtx;

/// Wrap one Rust source as an `RBS::Source::RBS` (or stub
/// `Source::Ruby`) Ruby instance using the supplied directives /
/// declarations arrays. The buffer is reused from the cached
/// per-source `RBS::Buffer` value so every `RBS::Location` from the
/// same source shares one Buffer.
pub fn build_source(
    ctx: &mut MaterializeCtx<'_>,
    src: &Source,
    directives: RArray,
    declarations: RArray,
) -> Result<Value, Error> {
    // M3 loader produces only RBS sources; the `Ruby` arm is reserved
    // for M5's `add_source` path. The Rust-side `SourceTag` enum has no
    // Ruby variant today, so dispatch is unconditional.
    let _ = src;
    let buffer = ctx.buffer()?;
    Ok(ctx
        .classes
        .source_rbs
        .new_instance((buffer, directives.as_value(), declarations.as_value()))?
        .as_value())
}
