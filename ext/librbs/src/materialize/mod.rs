//! Per-source materialization plumbing: cached Ruby class refs, lazy
//! `RBS::Buffer`, and the per-decl resolution cursor that lets later
//! slices pull `ResolvedRef`s from the [`Resolution`] side-table in
//! lockstep with the AST walk M3f–M3h will write.
//!
//! M3e wires only the plumbing — `MaterializeCtx` is constructed by the
//! temporary `_materialize_*` test entries and by the future
//! `materialize_all` cut-over (M3h). The actual per-node materialization
//! (types, members, decls) lands in M3f / M3g / M3h.

use magnus::{Error, RClass, Ruby, Value, kwargs, prelude::*, value::ReprValue};
use rustc_hash::FxHashMap;

use librbs_core::Environment;
use librbs_core::env::entry::DeclRef;
use librbs_core::env::resolution::{Resolution, ResolvedRef};
use librbs_core::interner::FrozenInterner;

pub mod decl;
pub mod directive;
pub mod location;
pub mod member;
pub mod method_type;
pub mod source;
pub mod type_;
pub mod type_name;
pub mod type_param;

/// Pre-resolved Ruby class refs used during materialization. Looking
/// these up once per [`MaterializeCtx`] avoids per-node `ruby.eval`s on
/// hot paths (M3f–M3h will instantiate `RBS::TypeName` / `RBS::Types::*`
/// / `RBS::AST::Members::*` hundreds of thousands of times for stdlib).
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
    pub entry_class: RClass,
    pub entry_module: RClass,
    pub entry_interface: RClass,
    pub entry_type_alias: RClass,
    pub entry_constant: RClass,
    pub entry_global: RClass,
    pub entry_class_alias: RClass,
    pub entry_module_alias: RClass,
    pub directives_use: RClass,
    pub directives_use_single_clause: RClass,
    pub directives_use_wildcard_clause: RClass,
    pub directives_resolve_type_names: RClass,
    pub source_rbs: RClass,
    /// Reserved for M5's Ruby-source path (loader does not produce Ruby
    /// sources today). Kept here so the M5 add_source patch only needs
    /// to wire dispatch, not class lookup.
    #[allow(dead_code)]
    pub source_ruby: RClass,
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
            entry_class: ruby.eval("RBS::Environment::ClassEntry")?,
            entry_module: ruby.eval("RBS::Environment::ModuleEntry")?,
            entry_interface: ruby.eval("RBS::Environment::InterfaceEntry")?,
            entry_type_alias: ruby.eval("RBS::Environment::TypeAliasEntry")?,
            entry_constant: ruby.eval("RBS::Environment::ConstantEntry")?,
            entry_global: ruby.eval("RBS::Environment::GlobalEntry")?,
            entry_class_alias: ruby.eval("RBS::Environment::ClassAliasEntry")?,
            entry_module_alias: ruby.eval("RBS::Environment::ModuleAliasEntry")?,
            directives_use: ruby.eval("RBS::AST::Directives::Use")?,
            directives_use_single_clause: ruby.eval("RBS::AST::Directives::Use::SingleClause")?,
            directives_use_wildcard_clause: ruby.eval("RBS::AST::Directives::Use::WildcardClause")?,
            directives_resolve_type_names: ruby.eval("RBS::AST::Directives::ResolveTypeNames")?,
            source_rbs: ruby.eval("RBS::Source::RBS")?,
            source_ruby: ruby.eval("RBS::Source::Ruby")?,
        })
    }
}

/// Per-source state threaded through every materialize helper.
///
/// The materializer iterates `env.*_decls`. For each decl it calls
/// [`enter_decl`] before walking the decl's AST subtree, which sets up
/// the `current_resolutions` slice from `resolution.get(decl_ref)`.
/// As the AST walk encounters type-name occurrences,
/// [`pull_resolution`] consumes from that slice in pre-order — the
/// same pre-order `resolver::driver::record_type_name` recorded in.
/// The materializer's walker (M3f–M3h) must mirror the driver's AST
/// traversal exactly; drift surfaces as a canonical-dump compat
/// failure in M3h.
pub struct MaterializeCtx<'a> {
    /// `&Ruby` is held so helpers (`type_name.rs`, `location.rs`) can
    /// allocate Ruby objects without re-acquiring the GVL handle. M3e
    /// callers reach for it; the field looks unused in M3e foundations
    /// because most helpers consume it through method calls on the
    /// `classes` ref instead.
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
    /// The source whose buffer / decls are currently being materialized.
    /// Switch via [`set_source`] when moving between sources within
    /// one materialization session — the [`buffers`] cache below
    /// survives the switch, so coming back to a previously-seen
    /// source re-uses the existing `RBS::Buffer` value (object
    /// identity, not just value equivalence).
    pub source_index: u32,
    pub classes: ClassRefs,
    /// Cached `RBS::Buffer` per source index. Built lazily on the
    /// first `make_location` call for that source and re-used
    /// thereafter so every `RBS::Location` from the same source
    /// shares one Buffer (upstream RBS uses Buffer identity in some
    /// equality checks). M3h's `materialize_all` creates a single
    /// `MaterializeCtx`, iterates `env.*_decls`, and switches
    /// `source_index` per decl — so even with N decls spread across
    /// M sources, only M Buffers are ever allocated.
    ///
    /// Cached `Value`s stay alive across calls because the very first
    /// `Location` built from a Buffer keeps it reachable from Ruby
    /// (the Location holds a reference, and the Location itself ends
    /// up attached to a materialized AST node which is in turn
    /// attached to `@class_decls` etc.). Without that anchor a GC
    /// run between cache miss and the next `make_location` could
    /// reclaim the Buffer; if M3h ever surfaces such a race, switch
    /// to a Ruby-side container (e.g. an `Array` ivar on the env)
    /// instead of this Rust-side map.
    buffers: FxHashMap<u32, Value>,
    /// Slice of resolutions for the decl currently being walked, set
    /// via [`enter_decl`]. `None` when the env was never resolved or
    /// when the current decl was filtered out by `only:` / a magic
    /// comment — the materializer falls back to raw AST names in
    /// either case.
    current_resolutions: Option<&'a [ResolvedRef]>,
    /// Index into `current_resolutions`, advanced by
    /// [`pull_resolution`].
    cursor: usize,
}

/// Saved cursor state for `MaterializeCtx`. M3h's nested-decl recursion
/// swaps the per-decl resolution slice when descending into a child decl
/// and restores the parent's slice on return — without this, a nested
/// `class Foo; class Bar; end; def baz: ...; end` would leak cursor
/// advancement from `Bar`'s slice into `Foo`'s, and the next
/// `pull_resolution` for `Foo` would consume an entry meant for `Bar`.
pub struct CursorState<'a> {
    current_resolutions: Option<&'a [ResolvedRef]>,
    cursor: usize,
    source_index: u32,
}

impl<'a> MaterializeCtx<'a> {
    pub fn new(
        ruby: &'a Ruby,
        env: &'a Environment,
        resolution: Option<&'a Resolution>,
        source_index: u32,
        classes: ClassRefs,
    ) -> Self {
        Self {
            ruby,
            env,
            interner: env.interner.frozen(),
            resolution,
            source_index,
            classes,
            buffers: FxHashMap::default(),
            current_resolutions: None,
            cursor: 0,
        }
    }

    /// Switch the active source. The buffer cache survives, so
    /// returning to a previously-active source picks up the same
    /// `RBS::Buffer` value. Used by M3h's `materialize_all` as it
    /// iterates `env.*_decls` and crosses source boundaries.
    pub fn set_source(&mut self, source_index: u32) {
        self.source_index = source_index;
    }

    /// Snapshot the resolution cursor state so a nested-decl recursion
    /// can [`enter_decl`] under a fresh slice and [`restore_cursor`]
    /// the outer decl's slice on return.
    pub fn save_cursor(&self) -> CursorState<'a> {
        CursorState {
            current_resolutions: self.current_resolutions,
            cursor: self.cursor,
            source_index: self.source_index,
        }
    }

    pub fn restore_cursor(&mut self, state: CursorState<'a>) {
        self.current_resolutions = state.current_resolutions;
        self.cursor = state.cursor;
        self.source_index = state.source_index;
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
    ///   bug between this module's walker (M3f–M3h) and
    ///   `resolver::driver`. M3h's canonical-dump compat tests are the
    ///   end-to-end regression guard.
    pub fn pull_resolution(&mut self) -> Option<ResolvedRef> {
        let slice = self.current_resolutions?;
        let r = slice.get(self.cursor).copied()?;
        self.cursor += 1;
        Some(r)
    }

    /// Lazily build (and cache) `RBS::Buffer.new(name:, content:)` for
    /// the current source. Subsequent calls for the same source
    /// return the same `Value` (object identity), and switching to a
    /// different source via [`set_source`] does not evict prior
    /// entries.
    pub fn buffer(&mut self) -> Result<Value, Error> {
        if let Some(buf) = self.buffers.get(&self.source_index) {
            return Ok(*buf);
        }
        let src = &self.env.sources[self.source_index as usize];
        let path_str = src.buffer.name.to_string_lossy().to_string();
        let pathname = self.classes.pathname.new_instance((path_str,))?.as_value();
        let content = src.buffer.content.as_str();
        let buffer = self
            .classes
            .buffer
            .new_instance((kwargs!("name" => pathname, "content" => content),))?
            .as_value();
        self.buffers.insert(self.source_index, buffer);
        Ok(buffer)
    }
}
