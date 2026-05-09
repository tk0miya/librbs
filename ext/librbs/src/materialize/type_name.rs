//! Build `RBS::TypeName` instances from interned [`TypeNameSym`]s.
//!
//! Two flavors:
//!
//! - [`materialize_type_name`] is the AST-as-written variant: it
//!   reflects the interned `(namespace, name)` pair exactly, marking
//!   the result `absolute!` only when the namespace is itself absolute.
//!   This is what M3f / M3g / M3h reach for when no resolution lookup
//!   is needed (e.g. a declaration's own name, which is not recorded in
//!   the [`Resolution`] table).
//!
//! - [`materialize_resolved_type_name`] is the resolution-aware variant:
//!   it pulls the next [`ResolvedRef`] from `MaterializeCtx`'s per-decl
//!   cursor (see `enter_decl`) and applies upstream's `absolute_type_name`
//!   semantics (`vendor/rbs/lib/rbs/environment.rb:982-985`).

use magnus::{Error, Value, kwargs, prelude::*, value::ReprValue};

use librbs_core::env::resolution::ResolvedRef;
use librbs_core::interner::TypeNameSym;

use crate::materialize::MaterializeCtx;

/// Build `RBS::TypeName` from the AST-interned `raw` symbol exactly as
/// written in the source. The result is marked `absolute!` only when
/// `raw`'s namespace is itself absolute (i.e. the source wrote
/// `::Foo`); relative names stay relative.
pub fn materialize_type_name(ctx: &MaterializeCtx<'_>, raw: TypeNameSym) -> Result<Value, Error> {
    let namespace_is_absolute = ctx
        .interner
        .namespaces()
        .lookup(ctx.interner.namespace_of(raw))
        .1;
    build_type_name_from_sym(ctx, raw, namespace_is_absolute)
}

/// Build `RBS::TypeName` consulting the per-decl resolution cursor on
/// [`MaterializeCtx`]. Branches mirror upstream's `absolute_type_name`
/// (`vendor/rbs/lib/rbs/environment.rb:982-985`):
///
/// - No resolution available — the env was never resolved, the current
///   decl was skipped by `only:` / a magic comment, or the cursor is
///   exhausted (parity bug): use `raw` as written, no `absolute!`.
/// - `Resolved(sym)`: build from `sym` and mark `absolute!`.
/// - `Unresolved(sym)`: build from `sym`, do not mark `absolute!`.
pub fn materialize_resolved_type_name(
    ctx: &mut MaterializeCtx<'_>,
    raw: TypeNameSym,
) -> Result<Value, Error> {
    match ctx.pull_resolution() {
        Some(ResolvedRef::Resolved(sym)) => build_type_name_from_sym(ctx, sym, true),
        Some(ResolvedRef::Unresolved(sym)) => {
            let absolute = ctx
                .interner
                .namespaces()
                .lookup(ctx.interner.namespace_of(sym))
                .1;
            build_type_name_from_sym(ctx, sym, absolute)
        }
        None => materialize_type_name(ctx, raw),
    }
}

fn build_type_name_from_sym(
    ctx: &MaterializeCtx<'_>,
    sym: TypeNameSym,
    mark_absolute: bool,
) -> Result<Value, Error> {
    let interner = ctx.interner;
    let (ns_sym, name_sym, _kind) = interner.lookup(sym);
    let (path_syms, _absolute) = interner.namespaces().lookup(ns_sym);

    let path_array = ctx.ruby.ary_new();
    for s in path_syms {
        let seg = interner.symbols().lookup(*s);
        path_array.push(ctx.ruby.to_symbol(seg))?;
    }
    let leaf = ctx.ruby.to_symbol(interner.symbols().lookup(name_sym));

    let namespace = ctx
        .classes
        .namespace
        .new_instance((kwargs!("path" => path_array, "absolute" => mark_absolute),))?
        .as_value();

    let type_name: Value = ctx
        .classes
        .type_name
        .new_instance((kwargs!("namespace" => namespace, "name" => leaf),))?
        .as_value();

    Ok(type_name)
}
