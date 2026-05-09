use std::path::PathBuf;
use std::sync::Arc;

use magnus::{
    Error, IntoValue, RArray, Ruby, Symbol, TryConvert, Value, function, prelude::*,
    value::ReprValue,
};
use rustc_hash::FxHashSet;

use librbs_core::env::resolution::Resolution;
use librbs_core::interner::{Sym, TypeNameInterner, TypeNameKind, TypeNameSym};
use ruby_rbs::node::{Node, TypeNameNode};

mod materialize;

use materialize::{ClassRefs, MaterializeCtx};

/// Magnus wrapper around `Arc<librbs_core::Environment>`. Boxed inside a
/// hidden Ruby class (`Librbs::Native::WrappedEnvironment`) and stashed on
/// each `RBS::Environment` instance via the `@__librbs_handle` ivar.
///
/// `Send + Sync` is required by `magnus::wrap`'s `TypedData` impl.
/// `Environment` is `Send + Sync` because every component
/// (`TypeNameInterner`, `Source`, `ManagedParser`) declares or derives
/// it; the `Arc` makes cloning the handle cheap when M3d / M3e need to
/// hand out additional references.
#[magnus::wrap(class = "Librbs::Native::WrappedEnvironment", free_immediately, size)]
#[allow(dead_code)]
struct WrappedEnvironment(Arc<librbs_core::Environment>);

impl WrappedEnvironment {
    fn arc(&self) -> &Arc<librbs_core::Environment> {
        &self.0
    }
}

/// Magnus wrapper around `Arc<librbs_core::env::resolution::Resolution>`.
/// M3c does not yet write `@__librbs_resolution`, but defining the class
/// here means M3d does not have to perform any registration churn — it
/// just `wrap`s an `Arc<Resolution>` and assigns the ivar.
#[magnus::wrap(class = "Librbs::Native::WrappedResolution", free_immediately, size)]
#[allow(dead_code)]
struct WrappedResolution(Arc<Resolution>);

impl WrappedResolution {
    fn new(resolution: Resolution) -> Self {
        Self(Arc::new(resolution))
    }
}

fn rb_runtime_err<E: std::fmt::Display>(e: E) -> Error {
    let ruby = Ruby::get().expect("Ruby thread");
    Error::new(ruby.exception_runtime_error(), e.to_string())
}

/// Direct `rb_ivar_get` via the `Object` trait. Avoids dispatching
/// through `instance_variable_get` so the canonical-dump path stays
/// inside the M3 "no Ruby method calls" invariant. `target` must be a
/// `T_OBJECT` (this is true of every `RBS::*` instance we touch).
fn ivar_get(target: Value, name: &str) -> Result<Value, Error> {
    let obj = magnus::RObject::try_convert(target)?;
    obj.ivar_get(name)
}

fn ivar_set(target: Value, name: &str, value: Value) -> Result<(), Error> {
    let obj = magnus::RObject::try_convert(target)?;
    obj.ivar_set(name, value)
}

/// Read `@core_root` from the Ruby loader and convert to a `PathBuf`.
fn read_core_root(loader: Value) -> Result<Option<PathBuf>, Error> {
    let v = ivar_get(loader, "@core_root")?;
    if v.is_nil() {
        return Ok(None);
    }
    // Pathname or String; both respond to `to_s`.
    let s: String = v.funcall("to_s", ())?;
    Ok(Some(PathBuf::from(s)))
}

/// Read `@dirs` (Array<Pathname>) from the Ruby loader.
fn read_dirs(loader: Value) -> Result<Vec<PathBuf>, Error> {
    let v = ivar_get(loader, "@dirs")?;
    if v.is_nil() {
        return Ok(Vec::new());
    }
    let arr = RArray::try_convert(v)?;
    let mut out = Vec::with_capacity(arr.len());
    for item in arr.into_iter() {
        let s: String = item.funcall("to_s", ())?;
        out.push(PathBuf::from(s));
    }
    Ok(out)
}

/// Read `@libs` (Set<Library>) from the Ruby loader. Each `Library` is a
/// `Struct.new(:name, :version, keyword_init: true)`.
fn read_libs(loader: Value) -> Result<Vec<(String, Option<String>)>, Error> {
    let v = ivar_get(loader, "@libs")?;
    if v.is_nil() {
        return Ok(Vec::new());
    }
    let arr: RArray = v.funcall("to_a", ())?;
    let mut out = Vec::with_capacity(arr.len());
    for lib in arr.into_iter() {
        let name: String = lib.funcall("name", ())?;
        let version_v: Value = lib.funcall("version", ())?;
        let version = if version_v.is_nil() {
            None
        } else {
            Some(String::try_convert(version_v)?)
        };
        out.push((name, version));
    }
    Ok(out)
}

/// Read `@repository.dirs` and call `Repository::add` on each.
fn read_repository(loader: Value, repo_out: &mut librbs_core::Repository) -> Result<(), Error> {
    let repo = ivar_get(loader, "@repository")?;
    if repo.is_nil() {
        return Ok(());
    }
    let dirs = ivar_get(repo, "@dirs")?;
    if dirs.is_nil() {
        return Ok(());
    }
    let arr = RArray::try_convert(dirs)?;
    for item in arr.into_iter() {
        let s: String = item.funcall("to_s", ())?;
        repo_out.add(PathBuf::from(s));
    }
    Ok(())
}

/// Mirror `RBS::EnvironmentLoader#load`'s stringio injection: when a
/// core_root is configured, `stringio` is added to `@libs` if it is not
/// already present. Replicating it here keeps the patched `from_loader`
/// (which only proxies to `build_environment`) byte-identical to pure
/// RBS without forcing the patch layer to reach into the loader.
fn inject_stringio(core_root: Option<&PathBuf>, libs: &mut Vec<(String, Option<String>)>) {
    if core_root.is_some() && !libs.iter().any(|(n, _)| n == "stringio") {
        libs.push(("stringio".to_string(), None));
    }
}

/// Read the input env's `@__librbs_handle` ivar and return the raw
/// Ruby value plus a `*mut Environment` pointing at the Arc-managed
/// allocation. The pointer is *not* derived from an `Arc::clone` —
/// keeping the strong count at 1 is what makes the unsafe mutation in
/// [`resolve_type_names`] sound (see the safety comment there).
///
/// Errors if the ivar is missing or wraps a foreign type — the
/// patched API only ever stores a `WrappedEnvironment` under that
/// name, so a missing handle indicates someone constructed an
/// `RBS::Environment` outside `from_loader` and tried to call
/// `resolve_type_names` on it.
fn extract_env_handle(env_ruby: Value) -> Result<(Value, *mut librbs_core::Environment), Error> {
    let handle = ivar_get(env_ruby, "@__librbs_handle")?;
    if handle.is_nil() {
        return Err(rb_runtime_err(
            "RBS::Environment has no @__librbs_handle; it must be built via Librbs::Native.build_environment",
        ));
    }
    let wrapped: &WrappedEnvironment = TryConvert::try_convert(handle)?;
    let env_ptr = Arc::as_ptr(wrapped.arc()) as *mut librbs_core::Environment;
    Ok((handle, env_ptr))
}

/// Convert a `RBS::Namespace` Ruby instance into a `(path, absolute)` pair
/// using ivar reads only. `@path` is an `Array<Symbol>`, `@absolute` is a
/// boolean.
fn read_namespace_components(ns: Value) -> Result<(Vec<String>, bool), Error> {
    let path_v = ivar_get(ns, "@path")?;
    let absolute_v = ivar_get(ns, "@absolute")?;
    let absolute = absolute_v.to_bool();
    let arr = RArray::try_convert(path_v)?;
    let mut path = Vec::with_capacity(arr.len());
    for seg in arr.into_iter() {
        let sym = Symbol::try_convert(seg)?;
        path.push(sym.name()?.into_owned());
    }
    Ok((path, absolute))
}

/// Intern a single `RBS::TypeName` instance into the supplied interner
/// using only ivar reads (no Ruby method calls). The kind tag is
/// determined from the leaf name's first character, mirroring
/// `TypeNameKind::detect`.
fn intern_ruby_type_name(
    interner: &mut TypeNameInterner,
    type_name: Value,
) -> Result<TypeNameSym, Error> {
    let ns_v = ivar_get(type_name, "@namespace")?;
    let name_v = ivar_get(type_name, "@name")?;
    let name_sym = Symbol::try_convert(name_v)?;
    let name_str = name_sym.name()?.into_owned();
    let (path_strs, absolute) = read_namespace_components(ns_v)?;

    let segs: Vec<Sym> = path_strs
        .iter()
        .map(|s| interner.symbols.intern(s))
        .collect();
    let ns = interner.namespaces.intern(&segs, absolute);
    let name = interner.symbols.intern(&name_str);
    let kind = TypeNameKind::detect(&name_str);
    Ok(interner.intern(ns, name, kind))
}

/// Convert the `only:` parameter — an `Array<RBS::TypeName>` (the
/// patched ruby side already turned the `Set` into an `Array`) or
/// `nil` — into an `Option<FxHashSet<TypeNameSym>>`. Returns `None` to
/// mean "resolve every declaration".
fn convert_only(
    interner: &mut TypeNameInterner,
    only: Value,
) -> Result<Option<FxHashSet<TypeNameSym>>, Error> {
    if only.is_nil() {
        return Ok(None);
    }
    let arr = RArray::try_convert(only)?;
    let mut out: FxHashSet<TypeNameSym> = FxHashSet::default();
    for tn in arr.into_iter() {
        out.insert(intern_ruby_type_name(interner, tn)?);
    }
    Ok(Some(out))
}

fn resolve_type_names(env_ruby: Value, only: Value) -> Result<Value, Error> {
    let ruby = Ruby::get().expect("Ruby thread");

    let (handle_value, env_ptr) = extract_env_handle(env_ruby)?;

    // Safety: we mutate the `Environment` behind a shared `Arc` via a raw
    // pointer rather than introducing interior mutability on the type.
    // This is sound under the following invariants, all of which hold for
    // the M3 patch-based design:
    //
    // 1. The `WrappedEnvironment` we read from `@__librbs_handle` owns
    //    the *only* `Arc<Environment>` clone for that allocation. No
    //    other Arc clones exist while we run because `extract_env_handle`
    //    deliberately does not call `Arc::clone`; it produces a raw
    //    pointer derived from the wrapped Arc itself. (`from_loader` is
    //    likewise the sole creator of the Arc.) Strong count is 1.
    // 2. Ruby's GVL serializes execution. No other Ruby thread can race
    //    with us, and no Ruby callback runs during `resolve` /
    //    `resolve_only` — they are pure Rust. The input env's ivar
    //    therefore cannot be observed in a half-mutated state.
    // 3. The `&mut Environment` we synthesize is local to this function
    //    and never escapes. It is dropped before we re-attach
    //    `handle_value` to the new env via `ivar_set`.
    //
    // M3e materialization will add an extra Arc clone path; before that
    // ships, this safety argument needs to be re-checked (and the
    // followup "Reimplement RBS::Environment in Rust" likely closes
    // this hatch entirely by giving the env interior mutability).
    let env: &mut librbs_core::Environment = unsafe { &mut *env_ptr };

    let only_set: Option<FxHashSet<TypeNameSym>> = convert_only(&mut env.interner, only)?;

    let resolution = librbs_core::resolver::driver::resolve(env, only_set.as_ref());

    // Allocate a fresh `RBS::Environment`. Pattern matches `build_environment`:
    // `allocate` + `send(:initialize)` so the upstream `@class_decls = {}`
    // etc. ivars exist for any future `super` call. (See the M3 followup
    // "Reimplement RBS::Environment in Rust" for why this allocate-and-
    // initialize dance survives in the patch-based design.)
    let rbs_env_class: magnus::RClass = ruby.eval("RBS::Environment")?;
    let rbs_env: Value = rbs_env_class.obj_alloc()?.as_value();
    let _: Value = rbs_env.funcall("send", (ruby.to_symbol("initialize"),))?;

    // Reuse the *same* `WrappedEnvironment` Ruby object on the new env so
    // `dst.@__librbs_handle.equal?(src.@__librbs_handle)` — the documented
    // "shared Arc<Environment>" invariant from the M3d task spec is
    // observable via Ruby object identity, not just Arc-target identity.
    ivar_set(rbs_env, "@__librbs_handle", handle_value)?;
    let wrapped_res: Value = WrappedResolution::new(resolution).into_value_with(&ruby);
    ivar_set(rbs_env, "@__librbs_resolution", wrapped_res)?;

    Ok(rbs_env)
}

/// Extract the optional `Resolution` from `@__librbs_resolution`.
///
/// Returns a raw `&'static Resolution` borrow obtained via `Arc::as_ptr`
/// — the lifetime is technically extended by the unsafe cast, but the
/// pointer is sound for the duration of the calling Ruby method because
/// (1) Ruby holds the env and its wrapper Arc on the stack and (2) the
/// GVL prevents concurrent mutation. Mirrors the borrow trick in
/// [`extract_env_handle`].
unsafe fn read_resolution_ptr(env_ruby: Value) -> Result<Option<*const Resolution>, Error> {
    let v = ivar_get(env_ruby, "@__librbs_resolution")?;
    if v.is_nil() {
        return Ok(None);
    }
    let wrapped: &WrappedResolution = TryConvert::try_convert(v)?;
    Ok(Some(Arc::as_ptr(&wrapped.0)))
}

// =====================================================================
// M3e temporary test-entry harness
//
// Everything in `m3e_test_entries` exists only so the M3e specs in
// `spec/unit/materialize_*` can exercise each plumbing layer without
// the rest of the materialize pipeline. The whole module — its private
// helpers (`first_decl_name`, `first_decl_super_name`) AND the
// `_materialize_*` singleton methods registered in `init` — is removed
// wholesale at M3h. See `docs/tasks/milestones/M3/M3h-decls-and-cutover.md`
// → "Cleanup".
// =====================================================================
mod m3e_test_entries {
    use super::*;

    /// First-decl name extractor: returns the name `TypeNameNode` of
    /// the first top-level declaration the source carries.
    /// Globals (which key by `Sym`, not `TypeNameSym`) are
    /// intentionally not handled — the spec inputs never use them.
    fn first_decl_name<'a>(decl: &Node<'a>) -> Option<TypeNameNode<'a>> {
        match decl {
            Node::Class(c) => Some(c.name()),
            Node::Module(m) => Some(m.name()),
            Node::Interface(i) => Some(i.name()),
            Node::TypeAlias(a) => Some(a.name()),
            Node::Constant(c) => Some(c.name()),
            Node::ClassAlias(a) => Some(a.new_name()),
            Node::ModuleAlias(a) => Some(a.new_name()),
            _ => None,
        }
    }

    /// First-decl super-class extractor: returns the AST node for the
    /// `< Foo` clause of the first `class` declaration, or `None` if
    /// the source's first decl is not a class or has no super_class.
    fn first_decl_super_name<'a>(decl: &Node<'a>) -> Option<TypeNameNode<'a>> {
        match decl {
            Node::Class(c) => c.super_class().map(|sc| sc.name()),
            _ => None,
        }
    }

    /// `Librbs::Native._materialize_first_class_name(env)`. Walks to
    /// the first declaration of the first source, materializes its
    /// name through [`materialize::type_name`], and returns the
    /// resulting `RBS::TypeName`.
    pub(super) fn materialize_first_class_name(env_ruby: Value) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let (_handle_value, env_ptr) = extract_env_handle(env_ruby)?;

        // Phase 1: with `&mut env`, intern the first decl's name. The
        // raw-pointer split mirrors `resolver::driver::resolve` — the
        // `&Source` borrow does not alias the `&mut Interner` borrow because
        // we never resize `env.sources` in this function.
        let raw: TypeNameSym = unsafe {
            let env_mut: &mut librbs_core::Environment = &mut *env_ptr;
            if env_mut.sources.is_empty() {
                return Err(rb_runtime_err("env has no sources"));
            }
            let src_ptr: *const librbs_core::Source = &env_mut.sources[0];
            let src: &librbs_core::Source = &*src_ptr;
            let first_decl = src
                .parser
                .signature()
                .declarations()
                .iter()
                .next()
                .ok_or_else(|| rb_runtime_err("source has no declarations"))?;
            let name_node = first_decl_name(&first_decl).ok_or_else(|| {
                rb_runtime_err("first decl has no materializable name (e.g. Global)")
            })?;
            librbs_core::env::insert::intern_type_name_node(&mut env_mut.interner, &name_node)
        };

        // Phase 2: with `&env`, build the MaterializeCtx and materialize.
        let env: &librbs_core::Environment = unsafe { &*env_ptr };
        let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
        let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
        let classes = ClassRefs::resolve(&ruby)?;
        let ctx = MaterializeCtx::new(&ruby, env, resolution, 0, classes);
        materialize::type_name::materialize_type_name(&ctx, raw)
    }

    /// `Librbs::Native._materialize_first_decl_location(env)`. Returns
    /// the `RBS::Location` for the entire first declaration of the
    /// first source. Used by the multi-byte regression fixture in
    /// `spec/unit/materialize_location_spec.rb`.
    pub(super) fn materialize_first_decl_location(env_ruby: Value) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let (_handle_value, env_ptr) = extract_env_handle(env_ruby)?;
        let env: &librbs_core::Environment = unsafe { &*env_ptr };
        let source = env
            .sources
            .first()
            .ok_or_else(|| rb_runtime_err("env has no sources"))?;
        let first_decl = source
            .parser
            .signature()
            .declarations()
            .iter()
            .next()
            .ok_or_else(|| rb_runtime_err("source has no declarations"))?;
        let range = first_decl.location();
        let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
        let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
        let classes = ClassRefs::resolve(&ruby)?;
        let mut ctx = MaterializeCtx::new(&ruby, env, resolution, 0, classes);
        materialize::location::make_location(&mut ctx, &range)
    }

    /// `Librbs::Native._materialize_all_decl_locations(env)`.
    /// Materializes the `RBS::Location` of every top-level declaration
    /// in the first source through a **single** `MaterializeCtx`,
    /// returning the list. Used by the buffer-sharing regression in
    /// `spec/unit/materialize_location_spec.rb` to assert that two
    /// Locations from the same source share `RBS::Buffer` object
    /// identity (not just value equivalence).
    pub(super) fn materialize_all_decl_locations(env_ruby: Value) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let (_handle_value, env_ptr) = extract_env_handle(env_ruby)?;
        let env: &librbs_core::Environment = unsafe { &*env_ptr };
        let source = env
            .sources
            .first()
            .ok_or_else(|| rb_runtime_err("env has no sources"))?;
        let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
        let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
        let classes = ClassRefs::resolve(&ruby)?;
        let mut ctx = MaterializeCtx::new(&ruby, env, resolution, 0, classes);
        let out = ruby.ary_new();
        for decl in source.parser.signature().declarations().iter() {
            let range = decl.location();
            let loc = materialize::location::make_location(&mut ctx, &range)?;
            out.push(loc)?;
        }
        Ok(out.as_value())
    }

    /// `Librbs::Native._materialize_first_super_name(env)`. Walks the
    /// first class declaration's super_class through
    /// [`materialize::type_name::materialize_resolved_type_name`],
    /// which consults the [`Resolution`] side-table when present.
    /// Returns `nil` when the first decl is not a class or has no
    /// super_class.
    pub(super) fn materialize_first_super_name(env_ruby: Value) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let (_handle_value, env_ptr) = extract_env_handle(env_ruby)?;

        let raw: Option<TypeNameSym> = unsafe {
            let env_mut: &mut librbs_core::Environment = &mut *env_ptr;
            if env_mut.sources.is_empty() {
                return Err(rb_runtime_err("env has no sources"));
            }
            let src_ptr: *const librbs_core::Source = &env_mut.sources[0];
            let src: &librbs_core::Source = &*src_ptr;
            let first_decl = src
                .parser
                .signature()
                .declarations()
                .iter()
                .next()
                .ok_or_else(|| rb_runtime_err("source has no declarations"))?;
            match first_decl_super_name(&first_decl) {
                Some(sn) => Some(librbs_core::env::insert::intern_type_name_node(
                    &mut env_mut.interner,
                    &sn,
                )),
                None => None,
            }
        };

        let raw = match raw {
            Some(r) => r,
            None => return Ok(ruby.qnil().as_value()),
        };

        let env: &librbs_core::Environment = unsafe { &*env_ptr };
        let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
        let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
        let classes = ClassRefs::resolve(&ruby)?;
        let mut ctx = MaterializeCtx::new(&ruby, env, resolution, 0, classes);
        // The first declaration of source 0 always has decl_index=0 (insert
        // numbers decls in pre-order from 0). Set up the resolution cursor
        // for that decl before pulling the super_class occurrence — which
        // for a `class Sub < Foo` source is the very first occurrence the
        // resolver pushed onto the slice.
        ctx.enter_decl(librbs_core::env::entry::DeclRef {
            source_index: 0,
            decl_index: 0,
        });
        materialize::type_name::materialize_resolved_type_name(&mut ctx, raw)
    }

    /// `Librbs::Native._materialize_first_type_alias_target(env)`. Walks
    /// to the first declaration of the first source — which must be a
    /// `type t = ...` alias — and returns the materialized
    /// `RBS::Types::*` instance for the alias's target. Compared to
    /// `RBS::Parser.parse_type` byte-for-byte in
    /// `spec/unit/materialize_type_spec.rb`.
    pub(super) fn materialize_first_type_alias_target(env_ruby: Value) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let (_handle_value, env_ptr) = extract_env_handle(env_ruby)?;
        let env: &librbs_core::Environment = unsafe { &*env_ptr };
        let source = env
            .sources
            .first()
            .ok_or_else(|| rb_runtime_err("env has no sources"))?;
        let first_decl = source
            .parser
            .signature()
            .declarations()
            .iter()
            .next()
            .ok_or_else(|| rb_runtime_err("source has no declarations"))?;
        let Node::TypeAlias(alias) = &first_decl else {
            return Err(rb_runtime_err("first decl is not a type alias"));
        };
        let target = alias.type_();
        let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
        let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
        let classes = ClassRefs::resolve(&ruby)?;
        let mut ctx = MaterializeCtx::new(&ruby, env, resolution, 0, classes);
        ctx.enter_decl(librbs_core::env::entry::DeclRef {
            source_index: 0,
            decl_index: 0,
        });
        materialize::type_::materialize_type(&mut ctx, &target)
    }

    /// `Librbs::Native._materialize_first_method_type_params(env)`.
    /// Walks the first class/module/interface declaration in source 0
    /// to its first `def`'s first overload, then materializes that
    /// `MethodType`'s `type_params` list as an `Array<RBS::AST::TypeParam>`.
    /// Compared against `RBS::Parser.parse_method_type` JSON in
    /// `spec/unit/materialize_type_param_spec.rb`.
    pub(super) fn materialize_first_method_type_params(env_ruby: Value) -> Result<Value, Error> {
        use ruby_rbs::node::{MethodTypeNode, TypeParamNode};

        let ruby = Ruby::get().expect("Ruby thread");
        let (_handle_value, env_ptr) = extract_env_handle(env_ruby)?;
        let env: &librbs_core::Environment = unsafe { &*env_ptr };
        let source = env
            .sources
            .first()
            .ok_or_else(|| rb_runtime_err("env has no sources"))?;
        // Walk top-level decls in source order, picking the first
        // class/module/interface that owns a method definition. The
        // `decl_counter` mirrors `resolver::driver`'s pre-order
        // increment so that the resulting `decl_index` is the same
        // one M3b's resolver associated the decl's resolution slice
        // with — required for the cursor lookup to find any nested
        // type-name occurrences.
        fn count_subtree(node: &Node<'_>) -> u32 {
            // 1 for `node` itself + recursive count of nested decls.
            let mut c = 1;
            let members = match node {
                Node::Class(cl) => Some(cl.members()),
                Node::Module(m) => Some(m.members()),
                _ => None,
            };
            if let Some(members) = members {
                for m in members.iter() {
                    // Inlined `is_decl_node`: only Class / Module /
                    // Interface / TypeAlias / Constant / Global /
                    // ClassAlias / ModuleAlias produce DeclRefs in
                    // `env::insert::insert_decl`. Mirroring that list
                    // here keeps `decl_counter` aligned with the
                    // resolver driver's pre-order numbering without
                    // widening `is_decl_node`'s public surface for a
                    // helper that's removed at M3h.
                    if matches!(
                        &m,
                        Node::Class(_)
                            | Node::Module(_)
                            | Node::Interface(_)
                            | Node::TypeAlias(_)
                            | Node::Constant(_)
                            | Node::Global(_)
                            | Node::ClassAlias(_)
                            | Node::ModuleAlias(_)
                    ) {
                        c += count_subtree(&m);
                    }
                }
            }
            c
        }
        let mut decl_counter: u32 = 0;
        let mut found = None;
        for decl in source.parser.signature().declarations().iter() {
            let members = match &decl {
                Node::Class(c) => Some(c.members()),
                Node::Module(m) => Some(m.members()),
                Node::Interface(i) => Some(i.members()),
                _ => None,
            };
            if let Some(members) = members
                && let Some(md) = members
                    .iter()
                    .find(|m| matches!(m, Node::MethodDefinition(_)))
            {
                found = Some((decl_counter, md));
                break;
            }
            decl_counter += count_subtree(&decl);
        }
        let (decl_index, method_def) =
            found.ok_or_else(|| rb_runtime_err("no method definition found in source"))?;
        let Node::MethodDefinition(md) = &method_def else {
            unreachable!();
        };
        let first_overload = md
            .overloads()
            .iter()
            .next()
            .ok_or_else(|| rb_runtime_err("method has no overloads"))?;
        let Node::MethodDefinitionOverload(overload) = &first_overload else {
            return Err(rb_runtime_err("overload node has unexpected variant"));
        };
        let mt_node = overload.method_type();
        let Node::MethodType(method_type) = &mt_node else {
            return Err(rb_runtime_err("overload's method_type is not a MethodType"));
        };
        // Borrow params as a typed handle so the iterator's lifetime
        // outlives the temporary `&MethodTypeNode`.
        let mt: &MethodTypeNode<'_> = method_type;
        let params = mt.type_params();

        let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
        let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
        let classes = ClassRefs::resolve(&ruby)?;
        let mut ctx = MaterializeCtx::new(&ruby, env, resolution, 0, classes);
        ctx.enter_decl(librbs_core::env::entry::DeclRef {
            source_index: 0,
            decl_index,
        });
        let arr = ruby.ary_new();
        for p in params.iter() {
            let Node::TypeParam(tp) = &p else {
                return Err(rb_runtime_err(
                    "type_params list contains non-TypeParam node",
                ));
            };
            let tp: &TypeParamNode<'_> = tp;
            arr.push(materialize::type_param::materialize_type_param(
                &mut ctx, tp,
            )?)?;
        }
        // Mirror the C parser's call to `RBS::AST::TypeParam.resolve_variables(type_params)`
        // so a TypeParam whose upper_bound mentions another TypeParam by
        // name (e.g. `[X < _Each[Y], Y]`) gets its inner Variable types
        // rewritten — required for byte-equivalence with
        // `RBS::Parser.parse_method_type`.
        let _: Value = ctx
            .classes
            .type_param
            .funcall("resolve_variables", (arr,))?;
        Ok(arr.as_value())
    }

    /// `Librbs::Native._materialize_first_class_type_params(env)`. Walks
    /// to the first class/module/interface/type-alias declaration in
    /// source 0 and materializes its declaration-level `type_params`
    /// list. Used by the M3f type_param spec to exercise variance and
    /// `unchecked` modifiers, both of which are only legal at the
    /// declaration level. Removed at M3h alongside the other
    /// `_materialize_*` entries.
    pub(super) fn materialize_first_class_type_params(env_ruby: Value) -> Result<Value, Error> {
        use ruby_rbs::node::TypeParamNode;

        let ruby = Ruby::get().expect("Ruby thread");
        let (_handle_value, env_ptr) = extract_env_handle(env_ruby)?;
        let env: &librbs_core::Environment = unsafe { &*env_ptr };
        let source = env
            .sources
            .first()
            .ok_or_else(|| rb_runtime_err("env has no sources"))?;
        let first_decl = source
            .parser
            .signature()
            .declarations()
            .iter()
            .next()
            .ok_or_else(|| rb_runtime_err("source has no declarations"))?;
        let params = match &first_decl {
            Node::Class(c) => c.type_params(),
            Node::Module(m) => m.type_params(),
            Node::Interface(i) => i.type_params(),
            Node::TypeAlias(a) => a.type_params(),
            _ => {
                return Err(rb_runtime_err(
                    "first decl has no declaration-level type_params",
                ));
            }
        };

        let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
        let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
        let classes = ClassRefs::resolve(&ruby)?;
        let mut ctx = MaterializeCtx::new(&ruby, env, resolution, 0, classes);
        ctx.enter_decl(librbs_core::env::entry::DeclRef {
            source_index: 0,
            decl_index: 0,
        });
        let arr = ruby.ary_new();
        for p in params.iter() {
            let Node::TypeParam(tp) = &p else {
                return Err(rb_runtime_err(
                    "type_params list contains non-TypeParam node",
                ));
            };
            let tp: &TypeParamNode<'_> = tp;
            arr.push(materialize::type_param::materialize_type_param(
                &mut ctx, tp,
            )?)?;
        }
        let _: Value = ctx
            .classes
            .type_param
            .funcall("resolve_variables", (arr,))?;
        Ok(arr.as_value())
    }
}
// =====================================================================
// End of M3e temporary test-entry harness
// =====================================================================

/// `Librbs::Native.materialize_all(env)` — M3e ships the **no-op stub**
/// of the entry point that M3h will fill in. Sets
/// `@__librbs_materialized = true` so the patched accessor methods'
/// `ensure_materialized` short-circuit works as intended even before
/// the cut-over (see [docs/tasks/milestones/M3-environment-and-resolver.md](
/// ../../docs/tasks/milestones/M3-environment-and-resolver.md) §
/// "materialize_all flow"). Idempotent: a second call is a fast no-op.
fn materialize_all(env_ruby: Value) -> Result<Value, Error> {
    let ruby = Ruby::get().expect("Ruby thread");
    let mat = ivar_get(env_ruby, "@__librbs_materialized")?;
    if mat.to_bool() {
        return Ok(ruby.qnil().as_value());
    }
    ivar_set(env_ruby, "@__librbs_materialized", ruby.qtrue().as_value())?;
    Ok(ruby.qnil().as_value())
}

fn build_environment(loader: Value) -> Result<Value, Error> {
    let ruby = Ruby::get().expect("Ruby thread");

    let core_root = read_core_root(loader)?;
    let mut libs = read_libs(loader)?;
    let dirs = read_dirs(loader)?;
    inject_stringio(core_root.as_ref(), &mut libs);

    let mut rust_loader = librbs_core::Loader::new();
    rust_loader.core_root = core_root;
    rust_loader.dirs = dirs;
    read_repository(loader, &mut rust_loader.repository)?;
    for (name, version) in libs {
        rust_loader.add_library(name, version);
    }

    let env = librbs_core::Environment::from_loader(&mut rust_loader).map_err(rb_runtime_err)?;
    let arc = Arc::new(env);

    // RBS::Environment.allocate, then send(:initialize) so the standard
    // `@class_decls = {}` etc. ivars exist (avoids `super` crashes when
    // M3e patches call super on accessor methods). Going through
    // `allocate` rather than `new` is necessary because we need to
    // attach `@__librbs_handle` *before* any user code observes the
    // instance.
    let rbs_env_class: magnus::RClass = ruby.eval("RBS::Environment")?;
    let rbs_env: Value = rbs_env_class.obj_alloc()?.as_value();
    let _: Value = rbs_env.funcall("send", (ruby.to_symbol("initialize"),))?;

    let wrapped: Value = WrappedEnvironment(arc).into_value_with(&ruby);
    ivar_set(rbs_env, "@__librbs_handle", wrapped)?;

    Ok(rbs_env)
}

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("Librbs")?.define_module("Native")?;

    // Hidden Ruby classes that back the magnus `wrap` macros. The
    // classes need to be registered before any instances are created;
    // they are intentionally kept under `Librbs::Native::` and not
    // surfaced publicly (the parent README forbids a public `Librbs::*`
    // API).
    let object = ruby.class_object();
    let wrapped_env_class = module.define_class("WrappedEnvironment", object)?;
    wrapped_env_class.undef_default_alloc_func();
    let wrapped_res_class = module.define_class("WrappedResolution", object)?;
    wrapped_res_class.undef_default_alloc_func();

    module.define_singleton_method("build_environment", function!(build_environment, 1))?;
    module.define_singleton_method("resolve_type_names", function!(resolve_type_names, 2))?;
    module.define_singleton_method("materialize_all", function!(materialize_all, 1))?;

    // Temporary M3e test entries; the entire `m3e_test_entries`
    // module (helpers + entry functions) is removed at M3h.
    module.define_singleton_method(
        "_materialize_first_class_name",
        function!(m3e_test_entries::materialize_first_class_name, 1),
    )?;
    module.define_singleton_method(
        "_materialize_first_decl_location",
        function!(m3e_test_entries::materialize_first_decl_location, 1),
    )?;
    module.define_singleton_method(
        "_materialize_all_decl_locations",
        function!(m3e_test_entries::materialize_all_decl_locations, 1),
    )?;
    module.define_singleton_method(
        "_materialize_first_super_name",
        function!(m3e_test_entries::materialize_first_super_name, 1),
    )?;
    module.define_singleton_method(
        "_materialize_first_type_alias_target",
        function!(m3e_test_entries::materialize_first_type_alias_target, 1),
    )?;
    module.define_singleton_method(
        "_materialize_first_method_type_params",
        function!(m3e_test_entries::materialize_first_method_type_params, 1),
    )?;
    module.define_singleton_method(
        "_materialize_first_class_type_params",
        function!(m3e_test_entries::materialize_first_class_type_params, 1),
    )?;

    Ok(())
}
