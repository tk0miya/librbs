//! Build `RBS::Source::RBS` instances from `librbs_core::Source`s.
//!
//! M3k cutover (PR Y2): for each source, materialise its buffer,
//! directives, and top-level decls in pre-order — then assemble a
//! `Source::RBS.new(buffer, directives, declarations)`. The
//! `materialize_all` driver passes each result to upstream
//! `RBS::Environment#add_source`, which handles `*_decls` indexing
//! identically to the pure-Ruby loader path. That keeps the
//! object-identity invariant
//! (`source.declarations[i].equal?(class_decls[name].decls[k].decl)`)
//! automatic, including for nested decls.
//!
//! Decl-walking lives in [`crate::materialize::decl`]; directive
//! materialisation lives in [`crate::materialize::directive`]. This
//! module is responsible only for `Source::RBS` assembly.

use magnus::{Error, Value, prelude::*, value::ReprValue};

use librbs_core::Source;

use crate::materialize::MaterializeCtx;
use crate::materialize::decl::materialize_declarations;
use crate::materialize::directive::materialize_directives;

/// Build the Ruby `RBS::Source::RBS` value for `source`. `source_index`
/// must match `source`'s position in `ctx.env.sources` — the buffer
/// installed on `ctx` is keyed off it, and the resolution cursor is
/// addressed by `(source_index, decl_index)` `DeclRef`s.
pub fn materialize_source_rbs(
    ctx: &mut MaterializeCtx<'_>,
    source_index: u32,
    source: &Source,
) -> Result<Value, Error> {
    // Install `source` as the active source: stores `source_index`
    // for nested-decl `DeclRef` assembly and eagerly builds the
    // `RBS::Buffer` so every `make_location` inside this source
    // shares one Ruby object.
    ctx.enter_source(source_index, source)?;

    let buffer = ctx.buffer();
    let signature = source.parser.signature();
    let directives =
        materialize_directives(ctx, signature.directives(), source.buffer.content.as_str())?;
    let declarations = materialize_declarations(ctx, signature.declarations())?;

    Ok(ctx
        .classes
        .source_rbs
        .new_instance((buffer, directives, declarations))?
        .as_value())
}
