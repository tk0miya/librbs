use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use magnus::{
    Error, IntoValue, RArray, Ruby, Symbol, TryConvert, Value, function, method, prelude::*,
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

/// State held by `Librbs::Native::Loader`. Everything that upstream's
/// `RBS::EnvironmentLoader` stores as plain ivars (`@core_root`,
/// `@libs`, `@dirs`) lives here, in Rust. The `@repository` ivar
/// stays on the Ruby side because it points at a Ruby
/// `RBS::Repository` instance — Rust never *owns* a Repository, it
/// just reads one through funcalls when it needs `repository.lookup`.
#[derive(Default)]
struct LoaderState {
    core_root: Option<String>,
    /// `(name, version)` pairs. Upstream uses `Set<Library>` keyed on
    /// the Struct's field-equality semantics; we replicate that by
    /// rejecting inserts whose `(name, version)` tuple matches an
    /// already-stored entry. Insertion order is preserved, matching
    /// Ruby `Set#each`'s insertion-order iteration.
    libs: Vec<(String, Option<String>)>,
    /// Paths added via `loader.add(path: ...)`. Stored as `String` so
    /// the Mutex'd state can stay `Send + Sync`; the Ruby wrapper
    /// rebuilds `Pathname` instances when callers ask for them.
    dirs: Vec<String>,
}

/// Magnus wrapper that backs `Librbs::Native::Loader`. Instances are
/// created from the Ruby side via
/// `Librbs::Patches::EnvironmentLoader#initialize`, stashed under
/// `@__librbs_loader`, and consulted by both the public Loader API
/// (add, libs, core_root, …) and the load/each_dir methods.
///
/// `Mutex` satisfies magnus's `Send + Sync` requirement on `wrap`ped
/// types. Under the GVL the lock is uncontended, so the cost is one
/// atomic per Ruby-visible method call.
#[magnus::wrap(class = "Librbs::Native::Loader", free_immediately, size)]
struct WrappedLoader(Mutex<LoaderState>);

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

impl WrappedLoader {
    /// `Librbs::Native::Loader.new(core_root_str)`. `core_root_str`
    /// is the upstream `core_root` Pathname pre-converted to a string
    /// (or `nil`); the actual path-string ↔ Pathname conversion stays
    /// on the Ruby side because Pathname is a Ruby object.
    fn new(core_root: Value) -> Self {
        let core_root: Option<String> = if core_root.is_nil() {
            None
        } else {
            TryConvert::try_convert(core_root).ok()
        };
        Self(Mutex::new(LoaderState {
            core_root,
            libs: Vec::new(),
            dirs: Vec::new(),
        }))
    }

    fn core_root(&self) -> Option<String> {
        self.0
            .lock()
            .expect("WrappedLoader mutex poisoned")
            .core_root
            .clone()
    }

    /// Return the stored `(name, version)` pairs as an
    /// `Array<[String, String|nil]>`. The Ruby wrapper turns this back
    /// into `Set[RBS::EnvironmentLoader::Library]` for the `libs`
    /// reader.
    fn libs(&self) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let state = self.0.lock().expect("WrappedLoader mutex poisoned");
        let out = ruby.ary_new();
        for (name, version) in &state.libs {
            let pair = ruby.ary_new();
            pair.push(name.clone().into_value_with(&ruby))?;
            match version {
                Some(v) => pair.push(v.clone().into_value_with(&ruby))?,
                None => pair.push(ruby.qnil().as_value())?,
            }
            out.push(pair)?;
        }
        Ok(out.as_value())
    }

    fn dirs(&self) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let state = self.0.lock().expect("WrappedLoader mutex poisoned");
        let out = ruby.ary_new();
        for dir in &state.dirs {
            out.push(dir.clone().into_value_with(&ruby))?;
        }
        Ok(out.as_value())
    }

    fn add_path(&self, path: String) {
        let mut state = self.0.lock().expect("WrappedLoader mutex poisoned");
        state.dirs.push(path);
    }

    /// `loader.add_lib(name, version, resolve_deps)`. `version` is
    /// `Value` (`String` or `nil`) so the caller doesn't have to
    /// pre-translate to `Option<String>`. If `resolve_deps` is true
    /// and the lib was newly inserted, fall back into Ruby's
    /// `Collection::Sources::{Rubygems,Stdlib}` to mirror upstream's
    /// `resolve_dependencies`.
    fn add_lib(&self, name: String, version: Value, resolve_deps: bool) -> Result<(), Error> {
        let version_str: Option<String> = if version.is_nil() {
            None
        } else {
            Some(TryConvert::try_convert(version)?)
        };

        // Phase 1: insert into libs, drop the mutex before any
        // re-entrant `add_lib` from `resolve_dependencies`.
        let inserted_new = {
            let mut state = self.0.lock().expect("WrappedLoader mutex poisoned");
            let entry = (name.clone(), version_str.clone());
            if state.libs.iter().any(|e| e == &entry) {
                false
            } else {
                state.libs.push(entry);
                true
            }
        };

        if inserted_new && resolve_deps {
            self.resolve_dependencies(&name, version_str.as_deref())?;
        }
        Ok(())
    }

    /// `resolve_dependencies` mirrors upstream's recursive
    /// `EnvironmentLoader#resolve_dependencies` by funcall-ing the
    /// `RBS::Collection::Sources::{Rubygems,Stdlib}` singletons. We
    /// don't replicate those sources in Rust (RubyGems-bound, same
    /// reasoning as `gem_sig_path`); the cost is funcalls per dep
    /// edge, which is sub-millisecond on real-world dependency trees.
    fn resolve_dependencies(&self, name: &str, version: Option<&str>) -> Result<(), Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let rubygems: Value = ruby.eval("RBS::Collection::Sources::Rubygems.instance")?;
        let stdlib: Value = ruby.eval("RBS::Collection::Sources::Stdlib.instance")?;

        for source in [&rubygems, &stdlib] {
            let has: bool = source.funcall("has?", (name, version))?;
            if !has {
                continue;
            }

            let effective_version: Option<String> = match version {
                Some(v) => Some(v.to_owned()),
                None => {
                    let versions: Value = source.funcall("versions", (name,))?;
                    let last: Value = versions.funcall("last", ())?;
                    if last.is_nil() {
                        return Err(rb_runtime_err(format!(
                            "no versions available for library {}",
                            name
                        )));
                    }
                    Some(last.funcall::<_, _, String>("to_s", ())?)
                }
            };

            let deps: Value =
                source.funcall("dependencies_of", (name, effective_version.clone()))?;
            if !deps.is_nil() {
                let arr = RArray::try_convert(deps)?;
                for dep in arr.into_iter() {
                    let dep_name: String = dep.funcall("[]", ("name",))?;
                    let nil_v: Value = ruby.qnil().as_value();
                    self.add_lib(dep_name, nil_v, true)?;
                }
            }
            return Ok(());
        }
        Ok(())
    }

    /// `loader.each_dir { |source, dir| ... }`. Yields per upstream
    /// `EnvironmentLoader#each_dir`'s contract: `:core` symbol for
    /// the core directory, `RBS::EnvironmentLoader::Library` for
    /// gem/repository-resolved libraries, and the user-supplied
    /// `Pathname` for paths added via `add(path: ...)`. Raises
    /// `RBS::EnvironmentLoader::UnknownLibraryError` carrying the
    /// `lib:` kwarg when a library resolves through neither
    /// `gem_sig_path` nor `repository.lookup`.
    ///
    /// `ruby_loader` is passed in explicitly because Rust needs to
    /// reach back for `loader.repository` — which is held as a Ruby
    /// ivar on the wrapper Loader, not as Rust state.
    fn each_dir(&self, ruby_loader: Value) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        if !ruby.block_given() {
            return Ok(ruby.qnil().as_value());
        }

        let env_loader_class: Value = ruby.eval("RBS::EnvironmentLoader")?;
        let library_class: Value = ruby.eval("RBS::EnvironmentLoader::Library")?;
        let unknown_lib_class: Value = ruby.eval("RBS::EnvironmentLoader::UnknownLibraryError")?;
        let pathname_class: Value = ruby.eval("Pathname")?;
        let core_sym: Value = ruby.to_symbol("core").as_value();

        let (core_root, libs, dirs) = {
            let state = self.0.lock().expect("WrappedLoader mutex poisoned");
            (
                state.core_root.clone(),
                state.libs.clone(),
                state.dirs.clone(),
            )
        };

        if let Some(core) = core_root {
            let core_path: Value = pathname_class.funcall("new", (core,))?;
            let _: Value = ruby.yield_values((core_sym, core_path))?;
        }

        let repository: Value = ruby_loader.funcall("repository", ())?;

        for (name, version) in &libs {
            let lib_value: Value = library_class.funcall(
                "new",
                (magnus::kwargs!(
                    "name" => name.clone(),
                    "version" => version.clone()
                ),),
            )?;

            let gem_result: Value =
                env_loader_class.funcall("gem_sig_path", (name.clone(), version.clone()))?;
            let path: Value = if !gem_result.is_nil() {
                let pair = RArray::try_convert(gem_result)?;
                pair.entry::<Value>(1)?
            } else {
                let repo_result: Value =
                    repository.funcall("lookup", (name.clone(), version.clone()))?;
                if !repo_result.is_nil() {
                    repo_result
                } else {
                    let err: magnus::Exception =
                        unknown_lib_class.funcall("new", (magnus::kwargs!("lib" => lib_value),))?;
                    return Err(Error::from(err));
                }
            };

            let _: Value = ruby.yield_values((lib_value, path))?;
        }

        for dir in &dirs {
            let path: Value = pathname_class.funcall("new", (dir.clone(),))?;
            let _: Value = ruby.yield_values((path, path))?;
        }

        Ok(ruby_loader)
    }

    /// `loader.load(ruby_loader, env)`. The Rust action that the
    /// `Librbs::Patches::EnvironmentLoader#load` patch ultimately
    /// dispatches to. Mirrors upstream's `EnvironmentLoader#load` —
    /// walks the directories `each_dir` would yield, runs the
    /// parallel `discovery → parse → entries` pipeline in Rust, and
    /// attaches the resulting `Arc<Environment>` to `env` via the
    /// `@__librbs_handle` ivar. Returns `env`.
    fn load(&self, ruby_loader: Value, env: Value) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let mut specs: Vec<librbs_core::discovery::DirSpec> = Vec::new();

        let (core_root, libs, dirs) = {
            let state = self.0.lock().expect("WrappedLoader mutex poisoned");
            (
                state.core_root.clone(),
                state.libs.clone(),
                state.dirs.clone(),
            )
        };

        if let Some(path) = core_root {
            specs.push(librbs_core::discovery::DirSpec {
                path: PathBuf::from(path),
                skip_hidden: true,
            });
        }

        let env_loader_class: Value = ruby.eval("RBS::EnvironmentLoader")?;
        let library_class: Value = ruby.eval("RBS::EnvironmentLoader::Library")?;
        let unknown_lib_class: Value = ruby.eval("RBS::EnvironmentLoader::UnknownLibraryError")?;
        let repository: Value = ruby_loader.funcall("repository", ())?;

        for (name, version) in &libs {
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

            let repo_result: Value =
                repository.funcall("lookup", (name.clone(), version.clone()))?;
            if !repo_result.is_nil() {
                let path: String = repo_result.funcall("to_s", ())?;
                specs.push(librbs_core::discovery::DirSpec {
                    path: PathBuf::from(path),
                    skip_hidden: true,
                });
                continue;
            }

            let lib_value: Value = library_class.funcall(
                "new",
                (magnus::kwargs!(
                    "name" => name.clone(),
                    "version" => version.clone()
                ),),
            )?;
            let err: magnus::Exception =
                unknown_lib_class.funcall("new", (magnus::kwargs!("lib" => lib_value),))?;
            return Err(Error::from(err));
        }

        for dir in &dirs {
            specs.push(librbs_core::discovery::DirSpec {
                path: PathBuf::from(dir.clone()),
                skip_hidden: false,
            });
        }

        let files = librbs_core::discovery::discover_rbs_files(specs).map_err(rb_runtime_err)?;
        let core_env = librbs_core::Environment::from_paths(files).map_err(rb_runtime_err)?;

        let wrapped: Value = WrappedEnvironment(Arc::new(core_env)).into_value_with(&ruby);
        ivar_set(env, "@__librbs_handle", wrapped)?;

        Ok(env)
    }
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

    // `Librbs::Native::Loader` is what `Librbs::Patches::EnvironmentLoader`
    // stashes under `@__librbs_loader`. The default alloc fn is kept
    // (magnus wires it up via `TypedData`) because the Ruby wrapper
    // calls `.new(core_root)` directly.
    let loader_class = module.define_class("Loader", object)?;
    loader_class.define_singleton_method("new", function!(WrappedLoader::new, 1))?;
    loader_class.define_method("core_root", method!(WrappedLoader::core_root, 0))?;
    loader_class.define_method("libs", method!(WrappedLoader::libs, 0))?;
    loader_class.define_method("dirs", method!(WrappedLoader::dirs, 0))?;
    loader_class.define_method("add_path", method!(WrappedLoader::add_path, 1))?;
    loader_class.define_method("add_lib", method!(WrappedLoader::add_lib, 3))?;
    loader_class.define_method("each_dir", method!(WrappedLoader::each_dir, 1))?;
    loader_class.define_method("load", method!(WrappedLoader::load, 2))?;

    module.define_singleton_method("resolve_type_names", function!(resolve_type_names, 2))?;
    module.define_singleton_method("materialize_all", function!(materialize_all, 1))?;

    Ok(())
}
