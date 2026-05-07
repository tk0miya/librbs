use std::path::PathBuf;

use magnus::{Error, Ruby, function, prelude::*};

fn build_environment_count(core_root: String) -> Result<usize, Error> {
    let mut loader = librbs_core::Loader::with_core_root(PathBuf::from(core_root));
    librbs_core::Environment::from_loader(&mut loader)
        .map(|env| env.class_decls.len())
        .map_err(|e| {
            let ruby = Ruby::get().expect("Ruby thread");
            Error::new(ruby.exception_runtime_error(), e.to_string())
        })
}

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("Librbs")?.define_module("Native")?;
    module.define_singleton_method(
        "build_environment_count",
        function!(build_environment_count, 1),
    )?;
    Ok(())
}
