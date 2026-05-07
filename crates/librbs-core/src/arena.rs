//! Arena façade. For M2 the arena is just a bump allocator wrapper; the
//! parsed RBS AST itself is owned by [`crate::source::ManagedParser`] and
//! doesn't yet copy into a Rust enum.

#[derive(Debug, Default)]
pub struct Arena;

impl Arena {
    pub fn new() -> Self {
        Self
    }
}
