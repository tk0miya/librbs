use std::path::PathBuf;
use std::sync::Arc;

use magnus::{
    Error, IntoValue, RArray, Ruby, Symbol, TryConvert, Value, function, prelude::*,
    value::ReprValue,
};
use rustc_hash::FxHashSet;

use librbs_core::env::resolution::Resolution;
use librbs_core::interner::{Sym, TypeNameInterner, TypeNameKind, TypeNameSym};

mod materialize;

use materialize::MaterializeCtx;

/// Magnus wrapper around `Arc<librbs_core::Environment>`. Boxed inside a
/// hidden Ruby class (`Librbs::Native::WrappedEnvironment`) and stashed on
/// each `RBS::Environment` instance via the `@__librbs_handle` ivar.
///
/// `Send + Sync` is required by `magnus::wrap`'s `TypedData` impl.
/// `Environment` is `Send + Sync` because every component
/// (`TypeNameInterner`, `Source`, `ManagedParser`) declares or derives
/// it; the `Arc` makes cloning the handle cheap when callers need to
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
/// inside the "no Ruby method calls" invariant. `target` must be a
/// `T_OBJECT` (this is true of every `RBS::*` instance we touch).
fn ivar_get(target: Value, name: &str) -> Result<Value, Error> {
    let obj = magnus::RObject::try_convert(target)?;
    obj.ivar_get(name)
}

fn ivar_set(target: Value, name: &str, value: Value) -> Result<(), Error> {
    let obj = magnus::RObject::try_convert(target)?;
    obj.ivar_set(name, value)
}

/// Read the input env's `@__librbs_handle` ivar and return the raw
/// Ruby value plus an `Arc::clone` of the wrapped `Environment`.
///
/// Errors if the ivar is missing or wraps a foreign type — the
/// patched API only ever stores a `WrappedEnvironment` under that
/// name, so a missing handle indicates someone constructed an
/// `RBS::Environment` outside `Librbs::Native.load_env` (the patched
/// `RBS::EnvironmentLoader#load`) and tried to call into the native
/// layer on it.
fn extract_env_handle(env_ruby: Value) -> Result<(Value, Arc<librbs_core::Environment>), Error> {
    let handle = ivar_get(env_ruby, "@__librbs_handle")?;
    if handle.is_nil() {
        return Err(rb_runtime_err(
            "RBS::Environment has no @__librbs_handle; it must be built via Librbs::Native.load_env (the patched RBS::EnvironmentLoader#load)",
        ));
    }
    let wrapped: &WrappedEnvironment = TryConvert::try_convert(handle)?;
    Ok((handle, Arc::clone(wrapped.arc())))
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

    let (_handle_value, src_arc) = extract_env_handle(env_ruby)?;

    // `Environment::clone` deep-copies the interner and the per-decl
    // tables and `Arc::clone`s the parsed sources (immutable for the
    // duration of resolve). The resolver mutates `env.interner` while
    // walking the AST; on the clone, those mutations land on the new
    // env only. The source env's `Arc<Environment>` is therefore never
    // mutated through here — it stays usable, un-resolved, identical
    // to upstream's `self`-is-unchanged contract for
    // `RBS::Environment#resolve_type_names`.
    let mut env: librbs_core::Environment = (*src_arc).clone();
    // Drop the source Arc as soon as the clone is in hand. Sources stay
    // alive through the Arc that `Environment::clone` cloned into `env`.
    drop(src_arc);

    let only_set: Option<FxHashSet<TypeNameSym>> = convert_only(&mut env.interner, only)?;

    let resolution = librbs_core::resolver::driver::resolve(&mut env, only_set.as_ref());

    // Allocate a fresh `RBS::Environment`: `allocate` + `send(:initialize)`
    // so the upstream `@class_decls = {}` etc. ivars exist for any future
    // `super` call from the patched accessors in
    // `lib/librbs/patches/environment.rb`.
    let rbs_env_class: magnus::RClass = ruby.eval("RBS::Environment")?;
    let rbs_env: Value = rbs_env_class.obj_alloc()?.as_value();
    let _: Value = rbs_env.funcall("send", (ruby.to_symbol("initialize"),))?;

    // Wrap the forked core env in a *fresh* `WrappedEnvironment` so
    // `src.@__librbs_handle` and `dst.@__librbs_handle` are no longer
    // `equal?`. The forked env owns its own `Arc<Environment>`; the
    // strong count is 1 here, which `materialize_all` still relies on
    // when it borrows the core env immutably.
    let dst_wrapped: Value = WrappedEnvironment(Arc::new(env)).into_value_with(&ruby);
    ivar_set(rbs_env, "@__librbs_handle", dst_wrapped)?;
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

/// `Librbs::Native.materialize_all(env)` — for each Rust source,
/// build a `RBS::Source::RBS` (buffer + directives + decls) and pass
/// it to upstream `RBS::Environment#add_source`. The upstream
/// `add_source` populates `@sources` plus the six `*_decls` ivars
/// using the same code path the pure-Ruby loader uses, so
/// `source.declarations[i].equal?(class_decls[name].decls[k].decl)`
/// holds at every nesting level by construction.
///
/// Idempotent: `@__librbs_materialized = true` is set before the
/// first `add_source` call so any patched accessor re-entered from
/// inside upstream's indexing short-circuits, and a second top-level
/// call returns immediately.
fn materialize_all(env_ruby: Value) -> Result<Value, Error> {
    let ruby = Ruby::get().expect("Ruby thread");
    let mat = ivar_get(env_ruby, "@__librbs_materialized")?;
    if mat.to_bool() {
        return Ok(ruby.qnil().as_value());
    }

    let (_handle_value, env_arc) = extract_env_handle(env_ruby)?;
    let env: &librbs_core::Environment = &env_arc;
    // SAFETY: see `read_resolution_ptr`'s doc — pointer remains valid
    // for the duration of this call because Ruby holds the env (and
    // thus the wrapper Arc) on the stack.
    let resolution_ptr = unsafe { read_resolution_ptr(env_ruby)? };
    let resolution: Option<&Resolution> = resolution_ptr.map(|p| unsafe { &*p });
    let classes = materialize::ClassRefs::resolve(&ruby)?;
    let mut ctx = MaterializeCtx::new(&ruby, env, resolution, classes);

    // Set the materialised flag *before* invoking `add_source` so any
    // patched accessor reached during upstream's indexing
    // (`add_source` → `insert_rbs_decl` → ivar reads on the env)
    // short-circuits past `ensure_materialized` instead of recursing.
    ivar_set(env_ruby, "@__librbs_materialized", ruby.qtrue().as_value())?;

    for (index, source) in env.sources.iter().enumerate() {
        let source_value =
            materialize::source::materialize_source_rbs(&mut ctx, index as u32, source.as_ref())?;
        let _: Value = env_ruby.funcall("add_source", (source_value,))?;
    }

    Ok(ruby.qnil().as_value())
}

/// `Librbs::Native.load_env(env, core_root, libs, dirs, repository)` —
/// invoked from the patched `RBS::EnvironmentLoader#load`. The Ruby
/// side hands over the loader's configuration; this function owns
/// the rest of the `each_dir` orchestration that upstream's pure-Ruby
/// `load` would run:
///
/// - `core_root`: `String` or `nil` (the core `.rbs` directory).
/// - `libs`: `Array<RBS::EnvironmentLoader::Library>` in insertion
///   order. Each lib is resolved by first calling back into Ruby for
///   `RBS::EnvironmentLoader.gem_sig_path(name, version)` and, on
///   `nil`, by calling `repository.lookup(name, version)`. Both
///   callbacks stay on the Ruby side: upstream's `Gem::Specification`
///   and `RBS::Repository::GemRBS` already memoize their per-gem
///   walks, so a Rust reimplementation was measured to be a wash
///   (single-load) or noise-level (multi-load) — see
///   `benchmark/summary.md` for the data behind the decision. If
///   neither callback yields a path, an
///   `RBS::EnvironmentLoader::UnknownLibraryError` is raised carrying
///   the original lib Value via the `lib:` kwarg.
/// - `dirs`: user-supplied paths added via `loader.add(path: ...)`.
///   These keep `skip_hidden = false` per upstream's
///   `!source.is_a?(Pathname)` rule.
/// - `repository`: the loader's `RBS::Repository` instance (upstream
///   class — librbs does not replace it). `repository.lookup` is the
///   only thing Rust calls on it; per-lib funcall overhead is sub-µs.
///
/// The resulting `(path, skip_hidden)` list is fed to
/// `librbs_core::discovery::discover_rbs_files` and then through
/// `librbs_core::Environment::from_paths` for the parallel read +
/// parse + `add_source` pipeline.
fn load_env(
    env: Value,
    core_root: Value,
    libs: RArray,
    dirs: RArray,
    repository: Value,
) -> Result<Value, Error> {
    let ruby = Ruby::get().expect("Ruby thread");

    let mut specs: Vec<librbs_core::discovery::DirSpec> = Vec::new();

    // 1. core
    if !core_root.is_nil() {
        let path: String = TryConvert::try_convert(core_root)?;
        specs.push(librbs_core::discovery::DirSpec {
            path: PathBuf::from(path),
            skip_hidden: true,
        });
    }

    // 2. libs — gem_sig_path callback into Ruby, fallback to
    // `RBS::Repository#lookup` (also Ruby).
    let env_loader_class: Value = ruby.eval("RBS::EnvironmentLoader")?;
    let unknown_lib_class: Value = ruby.eval("RBS::EnvironmentLoader::UnknownLibraryError")?;

    for lib in libs.into_iter() {
        let name: String = lib.funcall("name", ())?;
        let version_v: Value = lib.funcall("version", ())?;
        let version: Option<String> = if version_v.is_nil() {
            None
        } else {
            Some(TryConvert::try_convert(version_v)?)
        };

        let gem_result: Value =
            env_loader_class.funcall("gem_sig_path", (name.clone(), version.clone()))?;
        if !gem_result.is_nil() {
            let pair = RArray::try_convert(gem_result)?;
            let path_v: Value = pair.entry(1)?;
            let path: String = path_v.funcall("to_s", ())?;
            specs.push(librbs_core::discovery::DirSpec {
                path: PathBuf::from(path),
                skip_hidden: true,
            });
            continue;
        }

        let repo_result: Value = repository.funcall("lookup", (name.clone(), version.clone()))?;
        if !repo_result.is_nil() {
            let path: String = repo_result.funcall("to_s", ())?;
            specs.push(librbs_core::discovery::DirSpec {
                path: PathBuf::from(path),
                skip_hidden: true,
            });
            continue;
        }

        // Unknown library — raise the upstream-compatible class with
        // the lib Value attached, so consumers that `rescue
        // RBS::EnvironmentLoader::UnknownLibraryError => e; e.library`
        // keep working.
        let err_inst: magnus::Exception = env_loader_class
            .funcall::<_, _, Value>("const_get", (ruby.to_symbol("UnknownLibraryError"),))
            .and_then(|_| {
                unknown_lib_class
                    .funcall::<_, _, magnus::Exception>("new", (magnus::kwargs!("lib" => lib),))
            })?;
        return Err(Error::from(err_inst));
    }

    // 3. user-added dirs
    for d in dirs.into_iter() {
        let path: String = d.funcall("to_s", ())?;
        specs.push(librbs_core::discovery::DirSpec {
            path: PathBuf::from(path),
            skip_hidden: false,
        });
    }

    let files = librbs_core::discovery::discover_rbs_files(specs).map_err(rb_runtime_err)?;
    let core_env = librbs_core::Environment::from_paths(files).map_err(rb_runtime_err)?;

    let wrapped: Value = WrappedEnvironment(Arc::new(core_env)).into_value_with(&ruby);
    ivar_set(env, "@__librbs_handle", wrapped)?;

    Ok(env)
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

    module.define_singleton_method("load_env", function!(load_env, 5))?;
    module.define_singleton_method("resolve_type_names", function!(resolve_type_names, 2))?;
    module.define_singleton_method("materialize_all", function!(materialize_all, 1))?;

    Ok(())
}
