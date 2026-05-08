use std::path::PathBuf;
use std::sync::Arc;

use magnus::{
    Error, IntoValue, RArray, Ruby, TryConvert, Value, function, prelude::*, value::ReprValue,
};

use librbs_core::env::resolution::Resolution;

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

/// Magnus wrapper around `Arc<librbs_core::env::resolution::Resolution>`.
/// M3c does not yet write `@__librbs_resolution`, but defining the class
/// here means M3d does not have to perform any registration churn — it
/// just `wrap`s an `Arc<Resolution>` and assigns the ivar.
#[magnus::wrap(class = "Librbs::Native::WrappedResolution", free_immediately, size)]
#[allow(dead_code)]
struct WrappedResolution(Arc<Resolution>);

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
    Ok(())
}
