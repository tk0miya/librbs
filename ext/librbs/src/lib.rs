use std::path::PathBuf;
use std::sync::Arc;

use magnus::{
    Error, IntoValue, RArray, Ruby, Symbol, TryConvert, Value, function, prelude::*,
    value::ReprValue,
};
use rustc_hash::FxHashSet;

use librbs_core::env::entry::DeclRef;
use librbs_core::env::insert::is_decl_node;
use librbs_core::env::resolution::Resolution;
use librbs_core::interner::{Sym, TypeNameInterner, TypeNameKind, TypeNameSym};
use ruby_rbs::node::Node;

mod materialize;

use materialize::MaterializeCtx;

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

/// `Librbs::Native.materialize_all(env)` — walk every source's AST,
/// build the six entry hashes (`class_decls`, `interface_decls`,
/// `type_alias_decls`, `constant_decls`, `class_alias_decls`,
/// `global_decls`), and attach them to `env`'s upstream-visible ivars.
///
/// Idempotent: a second call observes `@__librbs_materialized = true`
/// and returns immediately so the six hashes preserve their object
/// identity across repeat accessor calls.
fn materialize_all(env_ruby: Value) -> Result<Value, Error> {
    let ruby = Ruby::get().expect("Ruby thread");
    let mat = ivar_get(env_ruby, "@__librbs_materialized")?;
    if mat.to_bool() {
        return Ok(ruby.qnil().as_value());
    }

    let (_handle_value, env_ptr) = extract_env_handle(env_ruby)?;
    // SAFETY: same as `resolve_type_names` — strong count is 1 on the
    // wrapped Arc, the GVL serializes Ruby threads, and the borrow
    // does not escape. We never resize `env.sources` below.
    let env: &librbs_core::Environment = unsafe { &*env_ptr };
    // SAFETY: see `read_resolution_ptr`'s doc — pointer remains valid
    // for the duration of this call because Ruby holds the env (and
    // thus the wrapper Arc) on the stack.
    let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
    let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
    let classes = materialize::ClassRefs::resolve(&ruby)?;
    let mut ctx = MaterializeCtx::new(&ruby, env, resolution, 0, classes);

    let hashes = materialize::decl::build_entries(&mut ctx)?;

    // Build `@sources` after the entry hashes — `top_level_decls` is
    // populated by `build_entries` and we re-use those Ruby decl values
    // here so `source.declarations[i].equal?(<entry's decl>)`.
    let sources_ary = ruby.ary_new();
    for (src_idx, src) in env.sources.iter().enumerate() {
        let src_idx = src_idx as u32;
        ctx.set_source(src_idx);
        let directives_ary = materialize::directive::build_directives(&mut ctx, src)?;
        let decls_ary = ruby.ary_new();
        let mut counter: u32 = 0;
        for top_decl in src.parser.signature().declarations().iter() {
            let decl_ref = DeclRef {
                source_index: src_idx,
                decl_index: counter,
            };
            if let Some(value) = hashes.top_level_decls.get(&decl_ref) {
                decls_ary.push(*value)?;
            }
            counter += count_decls_in_subtree(&top_decl);
        }
        let source_value =
            materialize::source::build_rbs_source(&mut ctx, src, directives_ary, decls_ary)?;
        sources_ary.push(source_value)?;
    }

    ivar_set(env_ruby, "@class_decls", hashes.class_decls.as_value())?;
    ivar_set(
        env_ruby,
        "@interface_decls",
        hashes.interface_decls.as_value(),
    )?;
    ivar_set(
        env_ruby,
        "@type_alias_decls",
        hashes.type_alias_decls.as_value(),
    )?;
    ivar_set(
        env_ruby,
        "@constant_decls",
        hashes.constant_decls.as_value(),
    )?;
    ivar_set(
        env_ruby,
        "@class_alias_decls",
        hashes.class_alias_decls.as_value(),
    )?;
    ivar_set(env_ruby, "@global_decls", hashes.global_decls.as_value())?;
    ivar_set(env_ruby, "@sources", sources_ary.as_value())?;
    ivar_set(env_ruby, "@__librbs_materialized", ruby.qtrue().as_value())?;

    Ok(ruby.qnil().as_value())
}

/// Pre-order count of decl nodes rooted at `node` — mirrors the
/// numbering `env::insert::insert_decl` and `resolver::driver::consume_decl_ref`
/// share. Used by `materialize_all` to advance through a source's
/// `signature().declarations()` while reading the per-source top-level
/// `DeclRef`s out of `EntryHashes::top_level_decls`.
fn count_decls_in_subtree(node: &Node<'_>) -> u32 {
    if !is_decl_node(node) {
        return 0;
    }
    let mut count: u32 = 1;
    match node {
        Node::Class(c) => {
            for member in c.members().iter() {
                count += count_decls_in_subtree(&member);
            }
        }
        Node::Module(m) => {
            for member in m.members().iter() {
                count += count_decls_in_subtree(&member);
            }
        }
        _ => {}
    }
    count
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

    Ok(())
}
