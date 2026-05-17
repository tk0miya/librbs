pub mod discovery;
pub mod env;
pub mod error;
pub mod interner;
pub mod node_kind;
pub mod repository;
pub mod resolver;
pub mod source;

pub use env::Environment;
pub use error::{Error, Result};
pub use source::Source;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
