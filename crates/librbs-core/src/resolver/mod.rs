//! Pure-Rust port of the resolution helpers under `RBS::Resolver`.
//!
//! Currently only the type-name resolver lives here; further resolvers
//! land alongside it as later milestones port them.

pub mod type_name;

pub use type_name::TypeNameResolver;
