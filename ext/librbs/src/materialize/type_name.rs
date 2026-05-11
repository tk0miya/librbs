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
use librbs_core::interner::{NamespaceSym, TypeNameSym};

use crate::materialize::MaterializeCtx;

/// Build `RBS::Namespace.new(path:, absolute:)` from an interned namespace
/// symbol. Shared between `build_type_name_from_sym` and the directive
/// materialiser (which needs a freestanding `RBS::Namespace` for
/// `Use::WildcardClause#namespace`).
///
/// Results are memoised on the per-run [`MaterializeCtx::ns_cache`] keyed
/// by `(ns_sym, absolute)` — common namespaces (`::`, `::Foo`, etc.)
/// appear in many materialised type names but only build their
/// `RBS::Namespace` Ruby instance once per run. See `cache.rs` for the
/// safety argument behind keeping the `Value` across calls.
pub fn materialize_namespace(
    ctx: &mut MaterializeCtx<'_>,
    ns_sym: NamespaceSym,
    absolute: bool,
) -> Result<Value, Error> {
    if ctx.cache_flags.ns
        && let Some(v) = ctx.ns_cache.get(ns_sym, absolute)
    {
        return Ok(v);
    }
    let path_array = path_array_for(ctx, ns_sym)?;
    let value = ctx
        .classes
        .namespace
        .new_instance((kwargs!("path" => path_array, "absolute" => absolute),))?
        .as_value();
    if ctx.cache_flags.ns {
        ctx.ns_cache.insert(ns_sym, absolute, value);
    }
    Ok(value)
}

/// Return a Ruby `Array<Symbol>` for the path of `ns_sym`. Cached on
/// [`MaterializeCtx::path_array_cache`] because the same path is used
/// for both `absolute=true` and `absolute=false` materialisations and
/// reappears across every `RBS::Namespace.new` call that shares the
/// path.
fn path_array_for(ctx: &mut MaterializeCtx<'_>, ns_sym: NamespaceSym) -> Result<Value, Error> {
    if ctx.cache_flags.path
        && let Some(v) = ctx.path_array_cache.get(ns_sym)
    {
        return Ok(v);
    }
    // Snapshot the path into an owned Vec so we can drop the borrow on
    // `ctx.interner` before calling `ctx.ruby_symbol_for` (which mutates
    // `ctx.sym_cache`).
    let path_syms: Vec<_> = ctx.interner.namespaces().lookup(ns_sym).0.clone();
    let arr = ctx.ruby.ary_new_capa(path_syms.len());
    for s in &path_syms {
        arr.push(ctx.ruby_symbol_for(*s))?;
    }
    let arr_value = arr.as_value();
    if ctx.cache_flags.path {
        ctx.path_array_cache.insert(ns_sym, arr_value);
    }
    Ok(arr_value)
}

/// Build `RBS::TypeName` from the AST-interned `raw` symbol exactly as
/// written in the source. The result is marked `absolute!` only when
/// `raw`'s namespace is itself absolute (i.e. the source wrote
/// `::Foo`); relative names stay relative.
pub fn materialize_type_name(
    ctx: &mut MaterializeCtx<'_>,
    raw: TypeNameSym,
) -> Result<Value, Error> {
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
    ctx: &mut MaterializeCtx<'_>,
    sym: TypeNameSym,
    mark_absolute: bool,
) -> Result<Value, Error> {
    if ctx.cache_flags.tn
        && let Some(v) = ctx.tn_cache.get(sym, mark_absolute)
    {
        return Ok(v);
    }
    let (ns_sym, name_sym, _kind) = ctx.interner.lookup(sym);
    let namespace = materialize_namespace(ctx, ns_sym, mark_absolute)?;
    let leaf = ctx.ruby_symbol_for(name_sym);
    let type_name: Value = ctx
        .classes
        .type_name
        .new_instance((kwargs!("namespace" => namespace, "name" => leaf),))?
        .as_value();
    if ctx.cache_flags.tn {
        ctx.tn_cache.insert(sym, mark_absolute, type_name);
    }
    Ok(type_name)
}
