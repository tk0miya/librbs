//! Per-source materialization plumbing: cached Ruby class refs, lazy
//! `RBS::Buffer`, and the per-decl resolution cursor that lets the AST
//! walk pull `ResolvedRef`s from the [`Resolution`] side-table in
//! lockstep with each declaration.

use std::cell::RefCell;
use std::ffi::c_char;
use std::os::raw::c_long;

use magnus::{
    Error, RClass, RHash, RObject, Ruby, Value, kwargs, prelude::*, value::Id, value::ReprValue,
};

use librbs_core::Environment;
use librbs_core::Source;
use librbs_core::env::DeclRef;
use librbs_core::env::resolution::{Resolution, ResolvedRef};
use librbs_core::interner::{FrozenInterner, NamespaceSym, Sym, TypeNameSym};

pub mod decl;
pub mod directive;
pub mod location;
pub mod member;
pub mod method_type;
mod rbs_extension_ffi;
pub mod source;
pub mod type_;
pub mod type_name;
pub mod type_param;

/// Pre-resolved Ruby class refs used during materialization. Looking
/// these up once per [`MaterializeCtx`] avoids per-node `ruby.eval`s on
/// hot paths (`RBS::TypeName` / `RBS::Types::*` / `RBS::AST::Members::*`
/// are instantiated hundreds of thousands of times for stdlib).
#[derive(Clone, Copy)]
pub struct ClassRefs {
    pub type_name: RClass,
    pub namespace: RClass,
    pub buffer: RClass,
    pub location: RClass,
    pub pathname: RClass,
    pub type_param: RClass,
    pub types_bases_bool: RClass,
    pub types_bases_void: RClass,
    pub types_bases_any: RClass,
    pub types_bases_nil: RClass,
    pub types_bases_top: RClass,
    pub types_bases_bottom: RClass,
    pub types_bases_self: RClass,
    pub types_bases_instance: RClass,
    pub types_bases_class: RClass,
    pub types_variable: RClass,
    pub types_literal: RClass,
    pub types_class_instance: RClass,
    pub types_interface: RClass,
    pub types_alias: RClass,
    pub types_class_singleton: RClass,
    pub types_tuple: RClass,
    pub types_union: RClass,
    pub types_intersection: RClass,
    pub types_record: RClass,
    pub types_optional: RClass,
    pub types_proc: RClass,
    pub types_block: RClass,
    pub types_function: RClass,
    pub types_function_param: RClass,
    pub types_untyped_function: RClass,
    pub method_type: RClass,
    pub annotation: RClass,
    pub comment: RClass,
    pub members_method_definition: RClass,
    pub members_attr_accessor: RClass,
    pub members_attr_reader: RClass,
    pub members_attr_writer: RClass,
    pub members_instance_variable: RClass,
    pub members_class_instance_variable: RClass,
    pub members_class_variable: RClass,
    pub members_include: RClass,
    pub members_extend: RClass,
    pub members_prepend: RClass,
    pub members_alias: RClass,
    pub members_public: RClass,
    pub members_private: RClass,
    pub decls_class: RClass,
    pub decls_class_super: RClass,
    pub decls_module: RClass,
    pub decls_module_self: RClass,
    pub decls_interface: RClass,
    pub decls_type_alias: RClass,
    pub decls_constant: RClass,
    pub decls_global: RClass,
    pub decls_class_alias: RClass,
    pub decls_module_alias: RClass,
    pub directives_use: RClass,
    pub directives_use_single_clause: RClass,
    pub directives_use_wildcard_clause: RClass,
    pub directives_resolve_type_names: RClass,
    pub source_rbs: RClass,
}

/// Pre-interned Ruby `Symbol` values for strings that show up across
/// the materializer with fixed text — method-definition kinds /
/// visibilities, attribute kinds, type-param variances, etc. Each
/// field stores a static `Symbol` interned via `rb_intern2` +
/// `rb_id2sym`, so the values are immortal (Ruby's static symbol
/// table never collects them) and safe to keep around as plain
/// `Value`s without GC tracking. Built once per [`MaterializeCtx`] in
/// [`CommonSyms::resolve`], read tens of thousands of times during
/// stdlib materialization.
#[derive(Clone, Copy)]
pub struct CommonSyms {
    pub instance: Value,
    pub singleton: Value,
    pub singleton_instance: Value,
    pub public: Value,
    pub private: Value,
    pub invariant: Value,
    pub covariant: Value,
    pub contravariant: Value,
    pub overload: Value,
    /// Pre-interned ivar ids used by the `obj_alloc + ivar_set` fast
    /// path. Stored as Ruby `Id`s (the raw `ID` `rb_intern2` returns)
    /// rather than `Symbol` values because `rb_ivar_set` consumes an
    /// `ID` directly — wrapping each as a `Symbol` would force a
    /// `rb_sym2id` round-trip at every use. The set covers every
    /// `@<field>` written by upstream `initialize` methods on classes
    /// the materializer fast-allocates (the `RBS::Types::*`,
    /// `RBS::MethodType`, `RBS::AST::Declarations::*` and
    /// `RBS::AST::Members::*` families). The `obj_alloc` path is
    /// only safe where upstream `initialize` is a pure sequence of
    /// `@x = x` assignments; see the per-class call sites in
    /// `materialize/type_.rs`, `materialize/method_type.rs`,
    /// `materialize/decl.rs`, and `materialize/member.rs`.
    pub ivar_location: Id,
    pub ivar_string: Id,
    pub ivar_name: Id,
    pub ivar_literal: Id,
    pub ivar_args: Id,
    pub ivar_types: Id,
    pub ivar_type: Id,
    pub ivar_block: Id,
    pub ivar_self_type: Id,
    pub ivar_return_type: Id,
    pub ivar_required_positionals: Id,
    pub ivar_optional_positionals: Id,
    pub ivar_rest_positionals: Id,
    pub ivar_trailing_positionals: Id,
    pub ivar_required_keywords: Id,
    pub ivar_optional_keywords: Id,
    pub ivar_rest_keywords: Id,
    pub ivar_required: Id,
    pub ivar_type_params: Id,
    pub ivar_super_class: Id,
    pub ivar_members: Id,
    pub ivar_annotations: Id,
    pub ivar_comment: Id,
    pub ivar_self_types: Id,
    pub ivar_new_name: Id,
    pub ivar_old_name: Id,
    pub ivar_kind: Id,
    pub ivar_overloads: Id,
    pub ivar_overloading: Id,
    pub ivar_visibility: Id,
    pub ivar_ivar_name: Id,
}

impl CommonSyms {
    pub fn resolve(ruby: &Ruby) -> Self {
        Self {
            instance: intern_symbol("instance"),
            singleton: intern_symbol("singleton"),
            singleton_instance: intern_symbol("singleton_instance"),
            public: intern_symbol("public"),
            private: intern_symbol("private"),
            invariant: intern_symbol("invariant"),
            covariant: intern_symbol("covariant"),
            contravariant: intern_symbol("contravariant"),
            overload: intern_symbol("Overload"),
            ivar_location: ruby.intern("@location"),
            ivar_string: ruby.intern("@string"),
            ivar_name: ruby.intern("@name"),
            ivar_literal: ruby.intern("@literal"),
            ivar_args: ruby.intern("@args"),
            ivar_types: ruby.intern("@types"),
            ivar_type: ruby.intern("@type"),
            ivar_block: ruby.intern("@block"),
            ivar_self_type: ruby.intern("@self_type"),
            ivar_return_type: ruby.intern("@return_type"),
            ivar_required_positionals: ruby.intern("@required_positionals"),
            ivar_optional_positionals: ruby.intern("@optional_positionals"),
            ivar_rest_positionals: ruby.intern("@rest_positionals"),
            ivar_trailing_positionals: ruby.intern("@trailing_positionals"),
            ivar_required_keywords: ruby.intern("@required_keywords"),
            ivar_optional_keywords: ruby.intern("@optional_keywords"),
            ivar_rest_keywords: ruby.intern("@rest_keywords"),
            ivar_required: ruby.intern("@required"),
            ivar_type_params: ruby.intern("@type_params"),
            ivar_super_class: ruby.intern("@super_class"),
            ivar_members: ruby.intern("@members"),
            ivar_annotations: ruby.intern("@annotations"),
            ivar_comment: ruby.intern("@comment"),
            ivar_self_types: ruby.intern("@self_types"),
            ivar_new_name: ruby.intern("@new_name"),
            ivar_old_name: ruby.intern("@old_name"),
            ivar_kind: ruby.intern("@kind"),
            ivar_overloads: ruby.intern("@overloads"),
            ivar_overloading: ruby.intern("@overloading"),
            ivar_visibility: ruby.intern("@visibility"),
            ivar_ivar_name: ruby.intern("@ivar_name"),
        }
    }
}

/// Intern `name` directly into a static Ruby `Symbol` via
/// `rb_intern2` + `rb_id2sym`. Cheaper than `Ruby::to_symbol`, which
/// builds an intermediate `RString` per call (the materializer hits
/// the same handful of static strings hundreds of thousands of times
/// for stdlib loads, so the per-call `RString` allocation is the bulk
/// of the cost there).
///
/// The returned `Symbol` is a static-symbol value, identical bit
/// pattern as `rb_id2sym(rb_intern2(...))` — Ruby's GC never frees
/// these, so callers can store the result in plain `Value` fields.
#[inline]
pub fn intern_symbol(name: &str) -> Value {
    // SAFETY: `rb_intern2` reads `len` bytes from `ptr`, hashes them
    // into Ruby's process-wide static symbol table, and returns a
    // stable `ID`. `rb_id2sym` is a pure bit-tagging operation. The
    // returned `VALUE` is a static-symbol immediate, which matches
    // the bit pattern `magnus::Value` expects.
    unsafe {
        let id = rb_sys::rb_intern2(name.as_ptr() as *const c_char, name.len() as c_long);
        rb_value_to_value(rb_sys::rb_id2sym(id))
    }
}

/// Write `value` to `@<id>` on `obj` via the bare `rb_ivar_set` path.
/// `obj` must come from `RClass::obj_alloc` on one of the
/// materializer's fast-allocated `RBS::*` classes — every such class
/// produces a `T_OBJECT`, so the inner `RObject::from_value` typecheck
/// always succeeds. The win over `new_instance(kwargs!(...))` is the
/// elimination of the kwargs `Hash` allocation and the `:initialize`
/// `funcall`; the only remaining work is a single `rb_ivar_set` per
/// field.
#[inline]
pub(crate) fn set_ivar(obj: Value, id: Id, value: Value) -> Result<(), Error> {
    let robj = RObject::from_value(obj).expect("obj_alloc must yield T_OBJECT");
    robj.ivar_set(id, value)
}

/// Reinterpret a raw `rb_sys::VALUE` as `magnus::Value`. The mirror of
/// `rbs_extension_ffi::raw_value`: `Value` is `#[repr(transparent)]`
/// over `(VALUE, PhantomData<...>)`, so the bit pattern is shared.
#[inline]
unsafe fn rb_value_to_value(v: rb_sys::VALUE) -> Value {
    // SAFETY: see above. `Value`'s size equals `VALUE`'s size; the
    // PhantomData has zero size.
    unsafe { std::mem::transmute_copy(&v) }
}

impl ClassRefs {
    pub fn resolve(ruby: &Ruby) -> Result<Self, Error> {
        Ok(Self {
            type_name: ruby.eval("RBS::TypeName")?,
            namespace: ruby.eval("RBS::Namespace")?,
            buffer: ruby.eval("RBS::Buffer")?,
            location: ruby.eval("RBS::Location")?,
            pathname: ruby.eval("Pathname")?,
            type_param: ruby.eval("RBS::AST::TypeParam")?,
            types_bases_bool: ruby.eval("RBS::Types::Bases::Bool")?,
            types_bases_void: ruby.eval("RBS::Types::Bases::Void")?,
            types_bases_any: ruby.eval("RBS::Types::Bases::Any")?,
            types_bases_nil: ruby.eval("RBS::Types::Bases::Nil")?,
            types_bases_top: ruby.eval("RBS::Types::Bases::Top")?,
            types_bases_bottom: ruby.eval("RBS::Types::Bases::Bottom")?,
            types_bases_self: ruby.eval("RBS::Types::Bases::Self")?,
            types_bases_instance: ruby.eval("RBS::Types::Bases::Instance")?,
            types_bases_class: ruby.eval("RBS::Types::Bases::Class")?,
            types_variable: ruby.eval("RBS::Types::Variable")?,
            types_literal: ruby.eval("RBS::Types::Literal")?,
            types_class_instance: ruby.eval("RBS::Types::ClassInstance")?,
            types_interface: ruby.eval("RBS::Types::Interface")?,
            types_alias: ruby.eval("RBS::Types::Alias")?,
            types_class_singleton: ruby.eval("RBS::Types::ClassSingleton")?,
            types_tuple: ruby.eval("RBS::Types::Tuple")?,
            types_union: ruby.eval("RBS::Types::Union")?,
            types_intersection: ruby.eval("RBS::Types::Intersection")?,
            types_record: ruby.eval("RBS::Types::Record")?,
            types_optional: ruby.eval("RBS::Types::Optional")?,
            types_proc: ruby.eval("RBS::Types::Proc")?,
            types_block: ruby.eval("RBS::Types::Block")?,
            types_function: ruby.eval("RBS::Types::Function")?,
            types_function_param: ruby.eval("RBS::Types::Function::Param")?,
            types_untyped_function: ruby.eval("RBS::Types::UntypedFunction")?,
            method_type: ruby.eval("RBS::MethodType")?,
            annotation: ruby.eval("RBS::AST::Annotation")?,
            comment: ruby.eval("RBS::AST::Comment")?,
            members_method_definition: ruby.eval("RBS::AST::Members::MethodDefinition")?,
            members_attr_accessor: ruby.eval("RBS::AST::Members::AttrAccessor")?,
            members_attr_reader: ruby.eval("RBS::AST::Members::AttrReader")?,
            members_attr_writer: ruby.eval("RBS::AST::Members::AttrWriter")?,
            members_instance_variable: ruby.eval("RBS::AST::Members::InstanceVariable")?,
            members_class_instance_variable: ruby
                .eval("RBS::AST::Members::ClassInstanceVariable")?,
            members_class_variable: ruby.eval("RBS::AST::Members::ClassVariable")?,
            members_include: ruby.eval("RBS::AST::Members::Include")?,
            members_extend: ruby.eval("RBS::AST::Members::Extend")?,
            members_prepend: ruby.eval("RBS::AST::Members::Prepend")?,
            members_alias: ruby.eval("RBS::AST::Members::Alias")?,
            members_public: ruby.eval("RBS::AST::Members::Public")?,
            members_private: ruby.eval("RBS::AST::Members::Private")?,
            decls_class: ruby.eval("RBS::AST::Declarations::Class")?,
            decls_class_super: ruby.eval("RBS::AST::Declarations::Class::Super")?,
            decls_module: ruby.eval("RBS::AST::Declarations::Module")?,
            decls_module_self: ruby.eval("RBS::AST::Declarations::Module::Self")?,
            decls_interface: ruby.eval("RBS::AST::Declarations::Interface")?,
            decls_type_alias: ruby.eval("RBS::AST::Declarations::TypeAlias")?,
            decls_constant: ruby.eval("RBS::AST::Declarations::Constant")?,
            decls_global: ruby.eval("RBS::AST::Declarations::Global")?,
            decls_class_alias: ruby.eval("RBS::AST::Declarations::ClassAlias")?,
            decls_module_alias: ruby.eval("RBS::AST::Declarations::ModuleAlias")?,
            directives_use: ruby.eval("RBS::AST::Directives::Use")?,
            directives_use_single_clause: ruby.eval("RBS::AST::Directives::Use::SingleClause")?,
            directives_use_wildcard_clause: ruby
                .eval("RBS::AST::Directives::Use::WildcardClause")?,
            directives_resolve_type_names: ruby.eval("RBS::AST::Directives::ResolveTypeNames")?,
            source_rbs: ruby.eval("RBS::Source::RBS")?,
        })
    }
}

/// Per-source state threaded through every materialize helper.
///
/// The materializer iterates `env.sources` (via `materialize_all`).
/// For each source it calls [`enter_source`] to install
/// `source_index` plus the source's `RBS::Buffer`; for each decl it
/// calls [`enter_decl`] before walking the decl's AST subtree, which
/// sets up the `current_resolutions` slice from
/// `resolution.get(decl_ref)`. As the AST walk encounters type-name
/// occurrences, [`pull_resolution`] consumes from that slice in
/// pre-order — the same pre-order `resolver::driver::record_type_name`
/// recorded in.
pub struct MaterializeCtx<'a> {
    /// `&Ruby` is held so helpers (`type_name.rs`, `location.rs`) can
    /// allocate Ruby objects without re-acquiring the GVL handle. Most
    /// helpers consume it through method calls on the `classes` ref
    /// instead, so the field can look unused at first glance.
    #[allow(dead_code)]
    pub ruby: &'a Ruby,
    pub env: &'a Environment,
    /// Read-only view of `env.interner`. Cached as a field so call
    /// sites can write `ctx.interner.lookup(...)` instead of
    /// `ctx.env.interner.frozen().lookup(...)` and so the read-only
    /// intent is expressed in the type. `FrozenInterner` is `Copy`,
    /// so storing it alongside the `&Environment` borrow costs
    /// nothing at runtime.
    pub interner: FrozenInterner<'a>,
    pub resolution: Option<&'a Resolution>,
    /// The source whose buffer / decls are currently being
    /// materialized. Set by [`enter_source`]; read by
    /// `materialize_nested_decl` when assembling per-decl
    /// `DeclRef`s.
    pub source_index: u32,
    pub classes: ClassRefs,
    /// `RBS::Buffer` for the current source. Built once by
    /// [`enter_source`] and reused for every `make_location` call
    /// inside that source — upstream RBS uses Buffer identity in
    /// some equality checks, so every `RBS::Location` from one
    /// source must share the same Ruby object. `None` before the
    /// first `enter_source` call, which the [`buffer`] accessor
    /// treats as a panic-worthy contract violation.
    ///
    /// The cached `Value` stays alive across calls because the
    /// first `Location` built from it keeps it reachable from Ruby
    /// (the Location holds a reference, and the Location itself
    /// ends up attached to a materialised AST node which is in turn
    /// reachable from `@sources` after `add_source`).
    current_buffer: Option<Value>,
    /// Slice of resolutions for the decl currently being walked, set
    /// via [`enter_decl`]. `None` when the env was never resolved or
    /// when the current decl was filtered out by `only:` / a magic
    /// comment — the materializer falls back to raw AST names in
    /// either case.
    current_resolutions: Option<&'a [ResolvedRef]>,
    /// Index into `current_resolutions`, advanced by
    /// [`pull_resolution`].
    cursor: usize,
    /// Flyweight table for materialised `RBS::Namespace` instances,
    /// keyed by `(NamespaceSym, absolute?)` packed into a single
    /// integer (see [`namespace_cache_key`]). Lets every reference to
    /// the same interned namespace share one Ruby object — stdlib
    /// materialisation hits common namespaces (`::`, `::RBS::`) tens
    /// of thousands of times.
    ///
    /// Stored as `RHash` (a Ruby object) rather than
    /// `HashMap<_, Value>` for GC safety: Ruby's collector marks values
    /// inside the hash via its standard hash-mark routine, while a
    /// Rust heap container would not be scanned. The `RHash` itself
    /// stays reachable because `MaterializeCtx` lives on the Rust
    /// stack of `materialize_all` and Ruby's conservative collector
    /// scans the C stack for VALUEs.
    namespace_cache: RHash,
    /// Flyweight table for materialised `RBS::TypeName` instances,
    /// keyed by `(TypeNameSym, mark_absolute)` (see
    /// [`type_name_cache_key`]). The `mark_absolute` bit must be part
    /// of the key because the same interned `TypeNameSym` can yield
    /// both an absolute and a relative `RBS::TypeName` depending on
    /// whether the call site is resolution-aware (resolved →
    /// `absolute!`) or raw (preserves AST flag).
    type_name_cache: RHash,
    /// Pre-interned static Ruby `Symbol`s for fixed strings used by
    /// the materializer (kind / visibility / variance values, the
    /// `Overload` constant lookup key, ...). See [`CommonSyms`].
    pub common: CommonSyms,
    /// Flyweight cache for interner-backed `Symbol`s. Each interner
    /// [`Sym`] (a `u32` assigned by `SymbolInterner`) maps one-to-one
    /// to a static Ruby symbol; this `Vec` indexed by `Sym.0` skips
    /// even Ruby's static-symbol-table hash lookup on the second hit.
    ///
    /// Stored as `RefCell<Vec<rb_sys::VALUE>>` (0 = uninitialized
    /// sentinel; `rb_id2sym` always produces a non-zero immediate
    /// tagged with `RUBY_SYMBOL_FLAG`). Plain `Vec` rather than a
    /// Ruby `RArray` because the cached `VALUE`s are static symbols —
    /// immortal — so Ruby's GC has nothing to scan and the extra
    /// write-barrier / boxing cost of `RArray.aset` is pure overhead
    /// for this access pattern.
    symbol_cache: RefCell<Vec<rb_sys::VALUE>>,
    /// Whether to use the `obj_alloc + ivar_set` fast path that
    /// bypasses upstream initializers. Snapshotted from
    /// [`fast_alloc_env`] at [`MaterializeCtx::new`] so the hot path
    /// reads a plain `bool` field instead of an atomic. The env var
    /// is read exactly once per process — see the function for the
    /// parsing rules.
    ///
    /// Currently gates the fast path on every materializer call site
    /// whose target class's upstream `initialize` is a pure sequence
    /// of `@x = x` assignments:
    ///
    /// - `RBS::Types::Bases::*` (`Bool`, `Void`, `Nil`, `Top`,
    ///   `Bottom`, `Self`, `Instance`, `Class`) and `Bases::Any`
    /// - `RBS::Types::Variable`, `Literal`, `ClassInstance`,
    ///   `Interface`, `Alias`, `ClassSingleton`, `Tuple`, `Union`,
    ///   `Intersection`, `Optional`, `Proc`, `Function`,
    ///   `UntypedFunction`, `Function::Param`, `Block`
    /// - `RBS::MethodType`
    /// - `RBS::AST::Declarations::Class` / `Module` / `Interface` /
    ///   `TypeAlias` / `Constant` / `Global` / `ClassAlias` /
    ///   `ModuleAlias`
    /// - `RBS::AST::Members::MethodDefinition` / `AttrAccessor` /
    ///   `AttrReader` / `AttrWriter` / `InstanceVariable` /
    ///   `ClassInstanceVariable` / `ClassVariable` / `Include` /
    ///   `Extend` / `Prepend` / `Alias` / `Public` / `Private`
    ///
    /// Classes whose upstream `initialize` does post-processing
    /// (`Types::Record` splits `all_fields` into `@fields` /
    /// `@optional_fields`) are intentionally excluded — adding them
    /// would require replicating the post-processing in Rust.
    pub fast_alloc: bool,
}

/// Read `LIBRBS_FAST_ALLOC` once and cache the answer. Default: fast
/// path enabled. Values that disable it: `0`, `false`, `off`, `no`
/// (case-insensitive). Everything else (including unset) keeps it on.
///
/// One switch gates every fast-path call site so downstream users
/// have a single knob to flip if upstream RBS ever changes the shape
/// of an `initialize` we've inlined. See the [`MaterializeCtx::fast_alloc`]
/// field comment for the full list of classes covered.
pub fn fast_alloc_env() -> bool {
    static CELL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *CELL.get_or_init(|| match std::env::var("LIBRBS_FAST_ALLOC") {
        Ok(s) => {
            let t = s.trim().to_ascii_lowercase();
            !matches!(t.as_str(), "0" | "false" | "off" | "no")
        }
        Err(_) => true,
    })
}

pub(crate) fn namespace_cache_key(ns: NamespaceSym, absolute: bool) -> i64 {
    ((ns.0 as i64) << 1) | (absolute as i64)
}

pub(crate) fn type_name_cache_key(tn: TypeNameSym, mark_absolute: bool) -> i64 {
    ((tn.0 as i64) << 1) | (mark_absolute as i64)
}

/// Saved cursor state for `MaterializeCtx`. The nested-decl recursion
/// swaps the per-decl resolution slice when descending into a child
/// decl and restores the parent's slice on return — without this, a
/// nested `class Foo; class Bar; end; def baz: ...; end` would leak
/// cursor advancement from `Bar`'s slice into `Foo`'s, and the next
/// `pull_resolution` for `Foo` would consume an entry meant for
/// `Bar`. `source_index` and `current_buffer` are not snapshotted —
/// nested decls always live in the same source as their parent.
pub struct CursorState<'a> {
    current_resolutions: Option<&'a [ResolvedRef]>,
    cursor: usize,
}

impl<'a> MaterializeCtx<'a> {
    pub fn new(
        ruby: &'a Ruby,
        env: &'a Environment,
        resolution: Option<&'a Resolution>,
        classes: ClassRefs,
    ) -> Self {
        let symbol_cache = vec![0 as rb_sys::VALUE; env.interner.frozen().symbols().len()];
        Self {
            ruby,
            env,
            interner: env.interner.frozen(),
            resolution,
            source_index: 0,
            classes,
            current_buffer: None,
            current_resolutions: None,
            cursor: 0,
            namespace_cache: ruby.hash_new(),
            type_name_cache: ruby.hash_new(),
            common: CommonSyms::resolve(ruby),
            symbol_cache: RefCell::new(symbol_cache),
            fast_alloc: fast_alloc_env(),
        }
    }

    /// Materialize an interner-backed [`Sym`] into a Ruby static
    /// `Symbol`, caching the result in `symbol_cache`. First call for
    /// a given `sym` calls into Ruby's static-symbol table; every
    /// subsequent call is a `Vec` index.
    #[inline]
    pub fn symbol_for(&self, sym: Sym) -> Value {
        let idx = sym.0 as usize;
        {
            let cache = self.symbol_cache.borrow();
            if let Some(&v) = cache.get(idx)
                && v != 0
            {
                // SAFETY: a non-zero slot was previously populated by
                // `intern_symbol`, which produced a static-symbol
                // `VALUE`. That bit pattern is a valid `Value`.
                return unsafe { rb_value_to_value(v) };
            }
        }
        let s = self.interner.symbols().lookup(sym);
        let value = intern_symbol(s);
        let mut cache = self.symbol_cache.borrow_mut();
        if idx >= cache.len() {
            // `intern()` (via `directive::materialize_use_clause`) can
            // grow the interner after `MaterializeCtx::new` snapshotted
            // its length; resize the cache lazily to cover those late
            // additions. Falls back to direct intern without caching
            // if the slot is still out of range somehow.
            cache.resize(idx + 1, 0);
        }
        // SAFETY: identical reinterpret as in `intern_symbol` —
        // `Value` and `VALUE` share representation.
        cache[idx] = unsafe { std::mem::transmute_copy::<Value, rb_sys::VALUE>(&value) };
        value
    }

    /// Intern an arbitrary `&str` directly via `rb_intern2` +
    /// `rb_id2sym`, bypassing magnus's `to_symbol` (which allocates
    /// an intermediate `RString`). Use this for strings that are not
    /// indexed by an interner `Sym`. See [`intern_symbol`] for the
    /// safety notes.
    #[inline]
    pub fn symbol_for_str(&self, name: &str) -> Value {
        intern_symbol(name)
    }

    /// Look up a previously materialised `RBS::Namespace` for the given
    /// `(NamespaceSym, absolute?)` pair, or `None` if this is the
    /// first request. The caller is expected to build the namespace on
    /// miss and call [`cache_namespace`] before returning.
    pub fn cached_namespace(&self, ns: NamespaceSym, absolute: bool) -> Option<Value> {
        self.namespace_cache.get(namespace_cache_key(ns, absolute))
    }

    pub fn cache_namespace(
        &self,
        ns: NamespaceSym,
        absolute: bool,
        value: Value,
    ) -> Result<(), Error> {
        self.namespace_cache
            .aset(namespace_cache_key(ns, absolute), value)
    }

    /// Look up a previously materialised `RBS::TypeName` for the given
    /// `(TypeNameSym, mark_absolute)` pair. The `mark_absolute` flag is
    /// keyed because the same `TypeNameSym` can appear both as a raw
    /// AST reference (preserving the AST's absolute flag) and as a
    /// resolved reference (forced to `absolute!`).
    pub fn cached_type_name(&self, tn: TypeNameSym, mark_absolute: bool) -> Option<Value> {
        self.type_name_cache
            .get(type_name_cache_key(tn, mark_absolute))
    }

    pub fn cache_type_name(
        &self,
        tn: TypeNameSym,
        mark_absolute: bool,
        value: Value,
    ) -> Result<(), Error> {
        self.type_name_cache
            .aset(type_name_cache_key(tn, mark_absolute), value)
    }

    /// Install `source` as the active source: store its `source_index`
    /// for nested-decl `DeclRef` assembly and eagerly build its
    /// `RBS::Buffer` so every `make_location` inside this source
    /// shares one Ruby object. Called once per source by
    /// [`crate::materialize::source::materialize_source_rbs`] before
    /// any AST walk for that source begins.
    pub fn enter_source(&mut self, source_index: u32, source: &Source) -> Result<(), Error> {
        self.source_index = source_index;
        let path_str = source.buffer.name.to_string_lossy().to_string();
        let pathname = self.classes.pathname.new_instance((path_str,))?.as_value();
        let content = source.buffer.content.as_str();
        let buffer = self
            .classes
            .buffer
            .new_instance((kwargs!("name" => pathname, "content" => content),))?
            .as_value();
        self.current_buffer = Some(buffer);
        Ok(())
    }

    /// Snapshot the resolution cursor state so a nested-decl recursion
    /// can [`enter_decl`] under a fresh slice and [`restore_cursor`]
    /// the outer decl's slice on return.
    pub fn save_cursor(&self) -> CursorState<'a> {
        CursorState {
            current_resolutions: self.current_resolutions,
            cursor: self.cursor,
        }
    }

    pub fn restore_cursor(&mut self, state: CursorState<'a>) {
        self.current_resolutions = state.current_resolutions;
        self.cursor = state.cursor;
    }

    /// Set the resolution slice for `decl_ref` as the cursor's source.
    /// Subsequent [`pull_resolution`] calls consume from this slice in
    /// pre-order. `current_resolutions` is `None` when the env was
    /// never resolved, when this decl was skipped by `only:` /
    /// `# resolve-type-names: false`, or simply when the decl had no
    /// type-name occurrences at all.
    pub fn enter_decl(&mut self, decl_ref: DeclRef) {
        self.current_resolutions = self.resolution.and_then(|r| r.get(decl_ref));
        self.cursor = 0;
    }

    /// Pull the next [`ResolvedRef`] for the current decl.
    ///
    /// Returns `None` when:
    ///
    /// - the env was never resolved (no [`Resolution`] attached),
    /// - the current decl was skipped during resolution and therefore
    ///   has no slice recorded in `Resolution`, or
    /// - the slice is exhausted — which only happens when the
    ///   materializer's walk over-shoots the resolver's, i.e. a parity
    ///   bug between this module's walker and `resolver::driver`. The
    ///   canonical-dump compat tests are the end-to-end regression
    ///   guard.
    pub fn pull_resolution(&mut self) -> Option<ResolvedRef> {
        let slice = self.current_resolutions?;
        let r = slice.get(self.cursor).copied()?;
        self.cursor += 1;
        Some(r)
    }

    /// Return the active source's `RBS::Buffer`. [`enter_source`]
    /// must have been called first; calling this before that is a
    /// programming error in the materialiser and panics.
    pub fn buffer(&self) -> Value {
        self.current_buffer
            .expect("MaterializeCtx::enter_source must be called before make_location")
    }
}
