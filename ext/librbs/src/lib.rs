use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use magnus::{
    DataTypeFunctions, Error, IntoValue, RArray, Ruby, Symbol, TryConvert, TypedData, Value,
    function, gc, method, prelude::*, value::Opaque, value::ReprValue,
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

/// `RBS::EnvironmentLoader::Library` mirror. Lives on the magnus
/// side because `Library` is conceptually Ruby-bound: instances are
/// constructed via `Library.new(name:, version:)` and surfaced
/// through `each_dir` and `UnknownLibraryError(lib:)`. The dedup
/// semantics are the same `Set<Library>` field-equality upstream
/// uses.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Library {
    name: String,
    version: Option<String>,
}

/// User-added sources of type information. Holds the two
/// fields that grow via `loader.add(...)` calls: directories the
/// user provided (`add(path: ...)`) and library entries
/// (`add(library: ...)`). `core_root` (system-provided default)
/// and `repository` (registry for resolving library names) sit
/// outside this struct on the magnus wrapper — they're not
/// "additions" the user accumulates the same way.
///
/// Lives behind a single `Mutex` so the two `Vec` fields can be
/// mutated through `&self` while still satisfying magnus's
/// `Send + Sync` requirement on the wrapper.
struct Additions {
    dirs: Vec<PathBuf>,
    libs: Vec<Library>,
}

/// Magnus wrapper for the public `RBS::EnvironmentLoader` class.
///
/// Unlike `WrappedEnvironment` / `WrappedResolution` (which are
/// hidden handles stashed on `RBS::Environment` ivars), this struct
/// *is* the user-facing class: `RBS::EnvironmentLoader` is set as
/// an alias for `Librbs::Native::EnvironmentLoader` by
/// `lib/librbs/patches/environment_loader.rb`, so the absence of a
/// `Wrapped*` prefix is intentional — `Wrapped*` is reserved for
/// internal handles.
///
/// Fields are split by mutability and semantic role:
///
/// - `core_root` — system-provided default type-information
///   directory. Immutable after construction; held outside the
///   `Mutex` so reads don't pay a lock acquisition.
/// - `additions` — user-added type-information sources (`dirs` +
///   `libs`). Mutable through `add(path:)` / `add(library:)`, so
///   it goes behind the `Mutex` to satisfy `Send + Sync`.
/// - `repository` — `Opaque<Value>` reference to a Ruby
///   `RBS::Repository` instance, used as a lookup registry for
///   resolving library `(name, version)` pairs to directories.
///   Held outside the `Mutex` because `Opaque<Value>` is already
///   `Sync`, and so that `DataTypeFunctions::mark` can mark it
///   without acquiring a lock during GC.
///
/// There is no pure-Rust loader type in `librbs-core` because the
/// loader's behaviour is uniformly Ruby-coupled: `Library`,
/// `UnknownLibraryError`, the `gem_sig_path` / `repository.lookup`
/// resolution chain, and the `each_dir` block protocol all require
/// Ruby callbacks. A pure `(core_root, dirs)` data holder would
/// have no logic on top of `Vec::push`, so it was dropped.
#[derive(TypedData)]
#[magnus(class = "Librbs::Native::EnvironmentLoader", free_immediately, mark)]
struct EnvironmentLoader {
    core_root: Option<PathBuf>,
    additions: Mutex<Additions>,
    repository: Opaque<Value>,
}

impl DataTypeFunctions for EnvironmentLoader {
    fn mark(&self, marker: &gc::Marker) {
        marker.mark(self.repository);
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

impl EnvironmentLoader {
    /// `Librbs::Native::EnvironmentLoader.new(core_root_str, repository)`
    /// — invoked from `lib/librbs/patches/environment_loader.rb`'s
    /// `new(core_root:, repository:)` wrapper after kwargs are
    /// destructured into positional args. `core_root_str` is
    /// `nil`-or-`String` (`Pathname#to_s` is already applied on the
    /// Ruby side); `repository` is the live `RBS::Repository`
    /// instance, held as `Opaque<Value>` so
    /// `DataTypeFunctions::mark` can mark it against GC.
    fn new(core_root: Option<String>, repository: Value) -> Self {
        Self {
            core_root: core_root.map(PathBuf::from),
            additions: Mutex::new(Additions {
                dirs: Vec::new(),
                libs: Vec::new(),
            }),
            repository: Opaque::from(repository),
        }
    }

    /// `loader.core_root` — returns `Pathname | nil`. The `Pathname`
    /// is constructed via funcall because it's a Ruby class; the
    /// underlying storage is `Option<PathBuf>` held directly on the
    /// wrapper (immutable after construction, no lock needed).
    fn core_root(&self) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        match &self.core_root {
            Some(path) => {
                let pathname_class: Value = ruby.eval("Pathname")?;
                let v: Value =
                    pathname_class.funcall("new", (path.to_string_lossy().into_owned(),))?;
                Ok(v)
            }
            None => Ok(ruby.qnil().as_value()),
        }
    }

    /// `loader.libs` — returns
    /// `Set[RBS::EnvironmentLoader::Library]` matching upstream's
    /// `attr_reader :libs`. The `Library` instances and the `Set`
    /// are constructed via funcall (they're Ruby types); the
    /// underlying storage is `Vec<Library>` on [`Additions`] with
    /// field-equality dedup.
    fn libs(&self) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        let library_class: Value = ruby.eval("RBS::EnvironmentLoader::Library")?;
        let set_class: Value = ruby.eval("Set")?;
        let set: Value = set_class.funcall("new", ())?;
        let additions = self
            .additions
            .lock()
            .expect("EnvironmentLoader additions mutex poisoned");
        for lib in &additions.libs {
            let lib_value: Value = library_class.funcall(
                "new",
                (magnus::kwargs!(
                    "name" => lib.name.clone(),
                    "version" => lib.version.clone()
                ),),
            )?;
            let _: Value = set.funcall("<<", (lib_value,))?;
        }
        Ok(set)
    }

    /// Reader for the held `RBS::Repository` instance. The
    /// `Opaque<Value>` round-trip needs a Ruby thread (`get_inner`).
    fn repository(&self) -> Value {
        let ruby = Ruby::get().expect("Ruby thread");
        ruby.get_inner(self.repository)
    }

    fn add_path(&self, path: String) {
        let mut additions = self
            .additions
            .lock()
            .expect("EnvironmentLoader additions mutex poisoned");
        additions.dirs.push(PathBuf::from(path));
    }

    /// Add a library entry. Returns the `inserted_new` flag so the
    /// Ruby side knows whether to recurse into
    /// `Collection::Sources::*.dependencies_of`. Dedup mirrors
    /// upstream's `Set#add?`: same `(name, version)` already
    /// present means no recursion.
    fn add_library(&self, name: String, version: Option<String>) -> Result<bool, Error> {
        let mut additions = self
            .additions
            .lock()
            .expect("EnvironmentLoader additions mutex poisoned");
        let new_entry = Library { name, version };
        if additions.libs.iter().any(|l| l == &new_entry) {
            Ok(false)
        } else {
            additions.libs.push(new_entry);
            Ok(true)
        }
    }

    /// Resolve a `Library` to its `.rbs` directory via the same
    /// chain upstream `EnvironmentLoader#each_dir` uses:
    /// `gem_sig_path(name, version)` first, then
    /// `repository.lookup(name, version)`. Returns `Ok(Some(path))`
    /// on success, `Ok(None)` if neither callback yields a path
    /// (caller raises `UnknownLibraryError` with the lib Value).
    fn resolve_library_dir(
        _ruby: &Ruby,
        env_loader_class: Value,
        repository: Value,
        lib: &Library,
    ) -> Result<Option<Value>, Error> {
        let gem_result: Value =
            env_loader_class.funcall("gem_sig_path", (lib.name.clone(), lib.version.clone()))?;
        if !gem_result.is_nil() {
            let pair = RArray::try_convert(gem_result)?;
            return Ok(Some(pair.entry::<Value>(1)?));
        }
        let repo_result: Value =
            repository.funcall("lookup", (lib.name.clone(), lib.version.clone()))?;
        if !repo_result.is_nil() {
            return Ok(Some(repo_result));
        }
        Ok(None)
    }

    /// `loader.each_dir { |source, dir| ... }` — yields per
    /// upstream's contract: `:core` symbol for the core directory,
    /// `RBS::EnvironmentLoader::Library` for gem/repository-resolved
    /// libraries, and the user-supplied `Pathname` for paths added
    /// via `add(path: ...)`. Iterates the three sources in their
    /// upstream order (core → libs → user dirs) as three sequential
    /// phases — the dedup-on-first-occurrence behaviour of
    /// `librbs_core::discovery::discover_rbs_files` depends on this
    /// order.
    fn each_dir(&self) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");
        if !ruby.block_given() {
            return Ok(ruby.qnil().as_value());
        }

        let env_loader_class: Value = ruby.eval("RBS::EnvironmentLoader")?;
        let library_class: Value = ruby.eval("RBS::EnvironmentLoader::Library")?;
        let unknown_library_error_class: Value =
            ruby.eval("RBS::EnvironmentLoader::UnknownLibraryError")?;
        let pathname_class: Value = ruby.eval("Pathname")?;
        let core_sym: Value = ruby.to_symbol("core").as_value();

        // Snapshot `additions` under the lock so we can drop the
        // mutex before yielding back to Ruby (which could otherwise
        // re-enter our methods). `core_root` lives outside the
        // mutex, so it's read directly off `self`.
        let (dirs, libs) = {
            let additions = self
                .additions
                .lock()
                .expect("EnvironmentLoader additions mutex poisoned");
            (additions.dirs.clone(), additions.libs.clone())
        };
        let repository: Value = ruby.get_inner(self.repository);

        // 1. core
        if let Some(path) = &self.core_root {
            let pathname: Value =
                pathname_class.funcall("new", (path.to_string_lossy().into_owned(),))?;
            let _: Value = ruby.yield_values((core_sym, pathname))?;
        }

        // 2. libs (between core and user dirs, matching upstream)
        Self::yield_libs(
            &ruby,
            env_loader_class,
            library_class,
            unknown_library_error_class,
            repository,
            &libs,
        )?;

        // 3. user dirs
        for path in &dirs {
            let pathname: Value =
                pathname_class.funcall("new", (path.to_string_lossy().into_owned(),))?;
            let _: Value = ruby.yield_values((pathname, pathname))?;
        }

        Ok(ruby.qnil().as_value())
    }

    fn yield_libs(
        ruby: &Ruby,
        env_loader_class: Value,
        library_class: Value,
        unknown_library_error_class: Value,
        repository: Value,
        libs: &[Library],
    ) -> Result<(), Error> {
        for lib in libs {
            let lib_value: Value = library_class.funcall(
                "new",
                (magnus::kwargs!(
                    "name" => lib.name.clone(),
                    "version" => lib.version.clone()
                ),),
            )?;

            let path = match Self::resolve_library_dir(ruby, env_loader_class, repository, lib)? {
                Some(p) => p,
                None => {
                    let err: magnus::Exception = unknown_library_error_class
                        .funcall("new", (magnus::kwargs!("lib" => lib_value),))?;
                    return Err(Error::from(err));
                }
            };

            let _: Value = ruby.yield_values((lib_value, path))?;
        }
        Ok(())
    }

    /// `loader.load_env(env)` — the Rust action that the Ruby
    /// `load(env:)` wrapper dispatches to. Walks the three sources
    /// in upstream's order (core → libs → user dirs), feeds the
    /// resulting `DirSpec` list to the parallel
    /// `discovery → parse → entries` pipeline, and attaches the
    /// resulting `Arc<Environment>` to `env` via the
    /// `@__librbs_handle` ivar.
    fn load_env(&self, env: Value) -> Result<Value, Error> {
        let ruby = Ruby::get().expect("Ruby thread");

        let (dirs, libs) = {
            let additions = self
                .additions
                .lock()
                .expect("EnvironmentLoader additions mutex poisoned");
            (additions.dirs.clone(), additions.libs.clone())
        };

        let env_loader_class: Value = ruby.eval("RBS::EnvironmentLoader")?;
        let library_class: Value = ruby.eval("RBS::EnvironmentLoader::Library")?;
        let unknown_library_error_class: Value =
            ruby.eval("RBS::EnvironmentLoader::UnknownLibraryError")?;
        let repository: Value = ruby.get_inner(self.repository);

        let mut specs: Vec<librbs_core::discovery::DirSpec> = Vec::new();

        // 1. core
        if let Some(path) = &self.core_root {
            specs.push(librbs_core::discovery::DirSpec {
                path: path.clone(),
                skip_hidden: true,
            });
        }

        // 2. libs
        Self::push_lib_specs(
            &ruby,
            env_loader_class,
            library_class,
            unknown_library_error_class,
            repository,
            &libs,
            &mut specs,
        )?;

        // 3. user dirs
        for path in &dirs {
            specs.push(librbs_core::discovery::DirSpec {
                path: path.clone(),
                skip_hidden: false,
            });
        }

        let files = librbs_core::discovery::discover_rbs_files(specs).map_err(rb_runtime_err)?;
        let core_env = librbs_core::Environment::from_paths(files).map_err(rb_runtime_err)?;

        let wrapped: Value = WrappedEnvironment(Arc::new(core_env)).into_value_with(&ruby);
        ivar_set(env, "@__librbs_handle", wrapped)?;

        Ok(env)
    }

    fn push_lib_specs(
        ruby: &Ruby,
        env_loader_class: Value,
        library_class: Value,
        unknown_library_error_class: Value,
        repository: Value,
        libs: &[Library],
        specs: &mut Vec<librbs_core::discovery::DirSpec>,
    ) -> Result<(), Error> {
        for lib in libs {
            match Self::resolve_library_dir(ruby, env_loader_class, repository, lib)? {
                Some(path_v) => {
                    let path: String = path_v.funcall("to_s", ())?;
                    specs.push(librbs_core::discovery::DirSpec {
                        path: PathBuf::from(path),
                        skip_hidden: true,
                    });
                }
                None => {
                    let lib_value: Value = library_class.funcall(
                        "new",
                        (magnus::kwargs!(
                            "name" => lib.name.clone(),
                            "version" => lib.version.clone()
                        ),),
                    )?;
                    let err: magnus::Exception = unknown_library_error_class
                        .funcall("new", (magnus::kwargs!("lib" => lib_value),))?;
                    return Err(Error::from(err));
                }
            }
        }
        Ok(())
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

    // `Librbs::Native::EnvironmentLoader` is the public class that
    // `lib/librbs/patches/environment_loader.rb` aliases to
    // `RBS::EnvironmentLoader`. The default alloc fn stays (magnus
    // wires it through `TypedData`) because the Ruby wrapper does
    // `__native_new__(core_root_str, repository)` to construct.
    let loader_class = module.define_class("EnvironmentLoader", object)?;
    loader_class.define_singleton_method("new", function!(EnvironmentLoader::new, 2))?;
    loader_class.define_method("core_root", method!(EnvironmentLoader::core_root, 0))?;
    loader_class.define_method("libs", method!(EnvironmentLoader::libs, 0))?;
    loader_class.define_method("repository", method!(EnvironmentLoader::repository, 0))?;
    loader_class.define_method("add_path", method!(EnvironmentLoader::add_path, 1))?;
    loader_class.define_method("add_library", method!(EnvironmentLoader::add_library, 2))?;
    loader_class.define_method("each_dir", method!(EnvironmentLoader::each_dir, 0))?;
    loader_class.define_method("load_env", method!(EnvironmentLoader::load_env, 1))?;

    module.define_singleton_method("resolve_type_names", function!(resolve_type_names, 2))?;
    module.define_singleton_method("materialize_all", function!(materialize_all, 1))?;

    Ok(())
}
