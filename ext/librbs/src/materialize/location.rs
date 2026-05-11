//! Helpers for constructing `RBS::Location` and its sub-locations from
//! parser-emitted [`RBSLocationRange`]s.
//!
//! Character offsets come straight from the parser via
//! `RBSLocationRange::start_char` / `end_char` (the local fork of
//! ruby-rbs adds these accessors); no byte → char conversion happens
//! on the Rust side. The prerequisite parser rewrite is what closes
//! the M2 followup "Byte ↔ character offset bridge for `RBS::Location`".
//!
//! Sub-location helpers prefer the dlsym FFI path into
//! `rbs_loc_legacy_add_required_child` /
//! `rbs_loc_legacy_add_optional_child` (see
//! [`rbs_extension_ffi`]), which calls the C entry points
//! directly with an `ID` and a two-`int` `rbs_loc_range` struct. That
//! skips Ruby method dispatch, `rb_check_typeddata`, `rb_sym2id`,
//! `NUM2INT`, and the Symbol allocation that the funcall path would
//! perform — for stdlib-sized loads the materialiser appends hundreds
//! of thousands of child entries, so the per-call savings add up.
//!
//! When the FFI symbols are unavailable (an rbs version that drops or
//! renames `rbs_loc_legacy_*`), each helper falls back to
//! `loc.funcall("_add_*_child", ...)` against the underscore-prefixed
//! Ruby primitives so the materialiser still produces correct output.
//!
//! [`add_required_child`] and [`add_optional_child`] both accept any
//! value implementing [`ChildRange`]. The parser-driven sites pass a
//! `RBSLocationRange` (returned by value from the bindgen accessors);
//! the magic-comment scanner in `directive::materialize_magic_comment`
//! passes a raw `(i32, i32)` tuple for its byte offsets (the
//! magic-comment grammar is ASCII so byte == char). The blanket
//! `impl<T: ChildRange> ChildRange for &T` also lets callers hand over
//! a borrow when they need to keep the range around afterwards
//! (e.g. attr-member helpers that read the same fields twice).

use magnus::{Error, Value, prelude::*, value::ReprValue};

use ruby_rbs::node::RBSLocationRange;

use crate::materialize::MaterializeCtx;
use crate::materialize::rbs_extension_ffi;

/// Anything that can be flattened to a `(start_char, end_char)` pair
/// for an `RBS::Location` child entry. The trait is the seam that
/// lets [`add_required_child`] / [`add_optional_child`] accept either
/// a parser `RBSLocationRange` or a raw `(i32, i32)` tuple at the
/// call site without forcing the caller to unpack upfront.
pub trait ChildRange {
    fn start_end(&self) -> (i32, i32);
}

impl ChildRange for RBSLocationRange {
    #[inline]
    fn start_end(&self) -> (i32, i32) {
        (self.start_char(), self.end_char())
    }
}

impl ChildRange for (i32, i32) {
    #[inline]
    fn start_end(&self) -> (i32, i32) {
        *self
    }
}

impl<T: ChildRange + ?Sized> ChildRange for &T {
    #[inline]
    fn start_end(&self) -> (i32, i32) {
        (**self).start_end()
    }
}

/// `RBS::Location.new(buffer, start_char, end_char)` for the current
/// source. Reads the active buffer from [`MaterializeCtx::buffer`].
pub fn make_location(ctx: &MaterializeCtx<'_>, range: &RBSLocationRange) -> Result<Value, Error> {
    let buffer = ctx.buffer();
    let start = range.start_char();
    let end = range.end_char();
    Ok(ctx
        .classes
        .location
        .new_instance((buffer, start, end))?
        .as_value())
}

/// Pre-size the location's children array to hold `cap` entries.
/// Without this hint the C side grows the array one slot at a time
/// (`realloc` per added child). Call this once with the exact total
/// number of children — required plus optional — before any
/// [`add_required_child`] / [`add_optional_child`] call on `loc`.
///
/// Silently no-ops if the upstream rbs C extension is older than the
/// version that exports `rbs_loc_legacy_alloc_children`; the
/// per-`_add_*_child` reallocation path still produces a correct
/// result.
pub fn alloc_children(_ctx: &MaterializeCtx<'_>, loc: Value, cap: u16) {
    rbs_extension_ffi::alloc_children(loc, cap);
}

/// Append a required sub-location at `name`. `range` can be a
/// [`RBSLocationRange`] (or borrow thereof) or a raw `(start, end)`
/// tuple — see [`ChildRange`].
pub fn add_required_child<R: ChildRange>(
    ctx: &MaterializeCtx<'_>,
    loc: Value,
    name: &str,
    range: R,
) -> Result<(), Error> {
    let (start, end) = range.start_end();
    if rbs_extension_ffi::try_add_required_child(loc, name, start, end) {
        return Ok(());
    }
    let sym = ctx.ruby.to_symbol(name);
    let _: Value = loc.funcall("_add_required_child", (sym, start, end))?;
    Ok(())
}

/// Append an optional sub-location at `name`. When `range` is `None`
/// the entry is marked present-but-empty (matching what
/// `add_optional_child(name, nil)` does upstream). `Some(_)` accepts
/// any [`ChildRange`].
pub fn add_optional_child<R: ChildRange>(
    ctx: &MaterializeCtx<'_>,
    loc: Value,
    name: &str,
    range: Option<R>,
) -> Result<(), Error> {
    match range {
        Some(r) => {
            let (start, end) = r.start_end();
            if rbs_extension_ffi::try_add_optional_child(loc, name, start, end) {
                return Ok(());
            }
            let sym = ctx.ruby.to_symbol(name);
            let _: Value = loc.funcall("_add_optional_child", (sym, start, end))?;
        }
        None => {
            if rbs_extension_ffi::try_add_optional_no_child(loc, name) {
                return Ok(());
            }
            let sym = ctx.ruby.to_symbol(name);
            let _: Value = loc.funcall("_add_optional_no_child", (sym,))?;
        }
    }
    Ok(())
}
