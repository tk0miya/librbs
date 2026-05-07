use magnus::{Error, Ruby, function, prelude::*};

fn hello() -> &'static str {
    "librbs alive"
}

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("Librbs")?.define_module("Native")?;
    module.define_singleton_method("hello", function!(hello, 0))?;
    Ok(())
}
