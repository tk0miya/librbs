use std::collections::HashMap;

use rayon::prelude::*;
use rustc_hash::FxHashSet;

pub mod entry;
pub mod insert;
pub mod resolution;
pub mod use_map;

pub use entry::{ClassAliasEntry, ClassLikeEntry};

use crate::discovery::Loader;
use crate::error::{Error, Result};
use crate::interner::{Sym, TypeNameInterner, TypeNameSym};
use crate::source::Source;

#[derive(Debug)]
pub struct Environment {
    pub interner: TypeNameInterner,
    pub sources: Vec<Source>,
    pub class_decls: HashMap<TypeNameSym, ClassLikeEntry>,
    pub interface_decls: FxHashSet<TypeNameSym>,
    pub type_alias_decls: FxHashSet<TypeNameSym>,
    pub constant_decls: FxHashSet<TypeNameSym>,
    pub class_alias_decls: HashMap<TypeNameSym, ClassAliasEntry>,
    pub global_decls: FxHashSet<Sym>,
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl Environment {
    pub fn new() -> Self {
        Self {
            interner: TypeNameInterner::new(),
            sources: Vec::new(),
            class_decls: HashMap::new(),
            interface_decls: FxHashSet::default(),
            type_alias_decls: FxHashSet::default(),
            constant_decls: FxHashSet::default(),
            class_alias_decls: HashMap::new(),
            global_decls: FxHashSet::default(),
        }
    }

    /// Build an environment from a `Loader`. Discovers files, parses them
    /// in parallel, then inserts entries serially.
    pub fn from_loader(loader: &mut Loader) -> Result<Self> {
        let files = loader.discover_files()?;

        // Parallel parse + IO.
        let sources: Vec<Source> = files
            .into_par_iter()
            .map(|(tag, path)| -> Result<Source> {
                let content = std::fs::read_to_string(&path).map_err(|e| Error::Io {
                    path: path.clone(),
                    source: e,
                })?;
                // Strip BOM if present.
                let content = content
                    .strip_prefix('\u{FEFF}')
                    .map(|s| s.to_string())
                    .unwrap_or(content);
                Source::new(tag, path.clone(), content)
                    .map_err(|message| Error::Parse { path, message })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut env = Environment::new();
        for src in sources.iter() {
            insert::insert_rbs_source(&mut env, src.parser.signature())?;
        }
        env.sources = sources;
        Ok(env)
    }
}
