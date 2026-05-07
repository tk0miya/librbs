use std::collections::HashMap;

use rayon::prelude::*;

pub mod entry;
pub mod insert;
pub mod use_map;

pub use entry::{
    ClassAliasLikeEntry, ClassLikeEntry, ConstantEntry, GlobalEntry, InterfaceEntry, TypeAliasEntry,
};

use crate::discovery::Loader;
use crate::error::{Error, Result};
use crate::interner::{Sym, TypeNameInterner, TypeNameSym};
use crate::source::Source;

#[derive(Debug)]
pub struct Environment {
    pub interner: TypeNameInterner,
    pub sources: Vec<Source>,
    pub class_decls: HashMap<TypeNameSym, ClassLikeEntry>,
    pub interface_decls: HashMap<TypeNameSym, InterfaceEntry>,
    pub type_alias_decls: HashMap<TypeNameSym, TypeAliasEntry>,
    pub constant_decls: HashMap<TypeNameSym, ConstantEntry>,
    pub class_alias_decls: HashMap<TypeNameSym, ClassAliasLikeEntry>,
    pub global_decls: HashMap<Sym, GlobalEntry>,
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
            interface_decls: HashMap::new(),
            type_alias_decls: HashMap::new(),
            constant_decls: HashMap::new(),
            class_alias_decls: HashMap::new(),
            global_decls: HashMap::new(),
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
        for (idx, src) in sources.iter().enumerate() {
            insert::insert_rbs_source(&mut env, idx as u32, src.parser.signature())?;
        }
        env.sources = sources;
        Ok(env)
    }
}
