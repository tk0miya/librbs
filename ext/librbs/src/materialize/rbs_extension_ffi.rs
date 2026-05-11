//! Best-effort FFI bridge into the upstream `rbs_extension.so`.
//!
//! Four functions in upstream rbs are useful enough to reach for
//! directly even though they are not part of any documented API:
//!
//! - `rbs_check_location(VALUE) -> rbs_loc*` extracts the C struct
//!   backing an `RBS::Location`.
//! - `rbs_loc_legacy_alloc_children(rbs_loc*, u16)` pre-sizes the
//!   `children` array on that struct. Without it, each child-add
//!   call grows the backing buffer by one entry via `realloc`.
//!   Pre-sizing once eliminates `N - 1` reallocs per location with
//!   `N` children.
//! - `rbs_loc_legacy_add_required_child(rbs_loc*, ID, rbs_loc_range)`
//!   appends a required sub-location entry.
//! - `rbs_loc_legacy_add_optional_child(rbs_loc*, ID, rbs_loc_range)`
//!   appends an optional sub-location entry. A range of `(-1, -1)`
//!   marks the entry as present-but-empty (the `no_child` case).
//!
//! Calling the C entry points directly skips the Ruby method
//! dispatch path used by `loc.funcall("_add_required_child", ...)`:
//! method lookup, `rb_check_typeddata`, `rb_sym2id`, `NUM2INT`, and
//! exception bookkeeping all go away. We extract the `rbs_loc*` once
//! per call via `rbs_check_location`, intern the child name with
//! `rb_intern2` (which is itself hash-cached inside Ruby), and pass
//! the `(start, end)` pair as a value-typed `rbs_loc_range` struct.
//!
//! All four symbols are non-static exports of `rbs_extension.so`. By
//! the time any Rust code in librbs runs, `require "rbs"` has already
//! loaded the extension with `RTLD_GLOBAL`, so the symbols are
//! reachable via `dlsym(RTLD_DEFAULT, ...)`.
//!
//! The names carry the `_legacy_` prefix upstream, signalling that
//! they may be replaced or renamed in a future rbs release. We treat
//! the bridge as best-effort: every public function in this module
//! returns whether the FFI path actually ran, and `location.rs`
//! falls back to the funcall path when it didn't. librbs keeps
//! working — only slower — against an rbs version that drops these
//! symbols.
//!
//! The `rbs_loc` pointer is treated as completely opaque on the Rust
//! side; we never dereference it. Only the C functions in
//! `rbs_extension.so` (which know the real layout) ever touch its
//! contents, so upstream is free to change the struct layout without
//! breaking us.

use std::ffi::{CStr, c_char, c_int, c_long, c_void};
use std::sync::OnceLock;

use magnus::Value;

/// Opaque sentinel for the C `rbs_loc*` pointer. Never dereferenced
/// on the Rust side; the only thing we do with it is hand it back to
/// other FFI calls.
#[repr(C)]
struct RbsLoc {
    _opaque: [u8; 0],
    _not_send_sync: std::marker::PhantomData<*mut u8>,
}

/// Mirrors `rbs_loc_range` from
/// `vendor/rbs/ext/rbs_extension/legacy_location.h` — two C `int`s,
/// passed by value. On x86_64 SysV that fits in a single register.
#[repr(C)]
#[derive(Clone, Copy)]
struct RbsLocRange {
    start: c_int,
    end: c_int,
}

/// Sentinel range used for "optional, no child" entries. Matches
/// `RBS_LOC_NULL_RANGE = { -1, -1 }` in upstream `legacy_location.c`.
const NULL_RANGE: RbsLocRange = RbsLocRange { start: -1, end: -1 };

type FnCheckLocation = unsafe extern "C" fn(rb_sys::VALUE) -> *mut RbsLoc;
type FnAllocChildren = unsafe extern "C" fn(*mut RbsLoc, u16);
type FnAddChild = unsafe extern "C" fn(*mut RbsLoc, rb_sys::ID, RbsLocRange);

struct Symbols {
    check_location: Option<FnCheckLocation>,
    alloc_children: Option<FnAllocChildren>,
    add_required_child: Option<FnAddChild>,
    add_optional_child: Option<FnAddChild>,
}

static SYMBOLS: OnceLock<Symbols> = OnceLock::new();

fn resolve() -> &'static Symbols {
    SYMBOLS.get_or_init(|| unsafe {
        Symbols {
            check_location: dlsym_fn(c"rbs_check_location"),
            alloc_children: dlsym_fn(c"rbs_loc_legacy_alloc_children"),
            add_required_child: dlsym_fn(c"rbs_loc_legacy_add_required_child"),
            add_optional_child: dlsym_fn(c"rbs_loc_legacy_add_optional_child"),
        }
    })
}

unsafe fn dlsym_fn<T: Copy>(name: &CStr) -> Option<T> {
    debug_assert_eq!(
        std::mem::size_of::<T>(),
        std::mem::size_of::<*mut c_void>(),
        "FFI function pointer type must be pointer-sized"
    );
    let ptr = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr() as *const c_char) };
    if ptr.is_null() {
        None
    } else {
        // SAFETY: caller guarantees `T` is a function-pointer type with
        // the same ABI as the symbol we just looked up. `dlsym`
        // returns a `void*` which on every supported platform is
        // bit-compatible with a function pointer.
        Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&ptr) })
    }
}

/// Extract the raw `rb_sys::VALUE` backing a magnus `Value`. magnus's
/// `Value` is `#[repr(transparent)]` over `(VALUE, PhantomData<...>)`,
/// so this is a no-op at the machine level. magnus does not expose a
/// public accessor for the same conversion.
#[inline]
fn raw_value(v: Value) -> rb_sys::VALUE {
    // SAFETY: `Value` and `rb_sys::VALUE` have identical layout per
    // magnus's `#[repr(transparent)]` annotation on `Value`.
    unsafe { std::mem::transmute_copy(&v) }
}

/// Intern `name` to a Ruby `ID`. Ruby caches IDs internally by name,
/// so repeated calls with the same string short-circuit to a hash
/// lookup.
#[inline]
fn intern(name: &str) -> rb_sys::ID {
    // SAFETY: `rb_intern2` reads `len` bytes starting at `ptr` and
    // does not allocate Ruby objects. The slice's pointer/length pair
    // is valid for the duration of the call.
    unsafe { rb_sys::rb_intern2(name.as_ptr() as *const c_char, name.len() as c_long) }
}

/// Pre-size the children array on the `rbs_loc` backing the given
/// `RBS::Location` value. Silently no-ops when either FFI symbol is
/// missing — callers should treat the call as a hint, not a
/// requirement.
pub fn alloc_children(loc: Value, cap: u16) {
    let syms = resolve();
    let (Some(check), Some(alloc)) = (syms.check_location, syms.alloc_children) else {
        return;
    };
    // SAFETY: `loc` is a valid Ruby VALUE held by the caller for the
    // duration of this call, so the GC will not move it. `check`
    // performs the TypedData type check internally and raises a
    // Ruby exception (longjmp) if `loc` is not an `RBS::Location`;
    // callers always pass values produced by `make_location` so the
    // check succeeds. The returned `rbs_loc*` lives as long as the
    // owning Ruby object, which the caller is holding.
    unsafe {
        let raw = check(raw_value(loc));
        if !raw.is_null() {
            alloc(raw, cap);
        }
    }
}

/// Append a required sub-location entry. Returns `true` when the FFI
/// path ran; `false` means the caller should fall back to the
/// `funcall` path.
pub fn try_add_required_child(loc: Value, name: &str, start: i32, end: i32) -> bool {
    let syms = resolve();
    let (Some(check), Some(add)) = (syms.check_location, syms.add_required_child) else {
        return false;
    };
    // SAFETY: see [`alloc_children`]. The additional argument `id`
    // comes from `rb_intern2`, which returns a stable `ID` for the
    // lifetime of the process.
    unsafe {
        let raw = check(raw_value(loc));
        if raw.is_null() {
            return false;
        }
        add(raw, intern(name), RbsLocRange { start, end });
    }
    true
}

/// Append an optional sub-location entry with a real range.
pub fn try_add_optional_child(loc: Value, name: &str, start: i32, end: i32) -> bool {
    let syms = resolve();
    let (Some(check), Some(add)) = (syms.check_location, syms.add_optional_child) else {
        return false;
    };
    // SAFETY: see [`try_add_required_child`].
    unsafe {
        let raw = check(raw_value(loc));
        if raw.is_null() {
            return false;
        }
        add(raw, intern(name), RbsLocRange { start, end });
    }
    true
}

/// Append an optional sub-location entry marked as present-but-empty.
/// Matches what `_add_optional_no_child(name)` does upstream.
pub fn try_add_optional_no_child(loc: Value, name: &str) -> bool {
    let syms = resolve();
    let (Some(check), Some(add)) = (syms.check_location, syms.add_optional_child) else {
        return false;
    };
    // SAFETY: see [`try_add_required_child`].
    unsafe {
        let raw = check(raw_value(loc));
        if raw.is_null() {
            return false;
        }
        add(raw, intern(name), NULL_RANGE);
    }
    true
}
