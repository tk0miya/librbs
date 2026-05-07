//! `UseMap` placeholder. Full porting lands in M3; this module exists so
//! `Environment` can expose the same module path it will retain later.

#[derive(Debug, Default)]
pub struct UseMap;

impl UseMap {
    pub fn new() -> Self {
        Self
    }
}
