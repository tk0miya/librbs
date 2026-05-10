use rustc_hash::{FxHashMap, FxHashSet};

use rayon::prelude::*;

pub mod insert;
pub mod resolution;
pub mod use_map;

use crate::discovery::Loader;
use crate::error::{Error, Result};
use crate::interner::{Sym, TypeNameInterner, TypeNameSym};
use crate::source::Source;

/// A reference to a particular declaration node, identifying its source
/// file and the index within that file's declaration list (the index is
/// pre-order over nested declarations).
///
/// Keys the per-decl `Resolution` side-table: the resolver assigns one
/// `DeclRef` per visited declaration in `env::insert::insert_rbs_source`
/// pre-order, and the materializer (in the `ext/librbs` crate) walks the
/// same order to read the recorded slice back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeclRef {
    pub source_index: u32,
    pub decl_index: u32,
}

/// `Resolver::context` equivalent: a chain of namespace nodes, leading from
/// the outermost (None) inward.
pub type Context = Vec<TypeNameSym>;

/// Payload for a class/module alias entry. The resolver reads
/// `(old_name, context)` to seed `TypeNameResolver::aliases`. The
/// `Class`/`Module` distinction upstream RBS makes between
/// `ClassAlias` and `ModuleAlias` is not consulted here, so a single
/// struct covers both.
#[derive(Debug)]
pub struct ClassAliasEntry {
    pub old_name: TypeNameSym,
    pub context: Context,
}

/// Unified entry stored in `Environment::decls` for every type-name
/// declaration. Class/module/interface/type-alias/constant variants
/// carry no payload — their identity is fully captured by the
/// `TypeNameSym` key (which already encodes `TypeNameKind`). Only
/// `ClassAlias` carries data, and is boxed so the enum stays small
/// (one word for the tag + one word for the pointer).
#[derive(Debug)]
pub enum DeclEntry {
    Class,
    Module,
    Interface,
    TypeAlias,
    Constant,
    ClassAlias(Box<ClassAliasEntry>),
}

impl DeclEntry {
    /// Whether this decl contributes a name to the resolver's
    /// "all known type names" set. Class/module/interface/type-alias
    /// names are resolvable; constants and class aliases are not
    /// (aliases feed `TypeNameResolver::aliases` separately).
    pub fn is_resolvable(&self) -> bool {
        matches!(
            self,
            DeclEntry::Class | DeclEntry::Module | DeclEntry::Interface | DeclEntry::TypeAlias
        )
    }
}

#[derive(Debug)]
pub struct Environment {
    pub interner: TypeNameInterner,
    pub sources: Vec<Source>,
    /// All type-name declarations (class, module, interface, type alias,
    /// constant, class/module alias) keyed by their interned absolute
    /// `TypeNameSym`. The variant disambiguates the kind; payload only
    /// exists for aliases.
    pub decls: FxHashMap<TypeNameSym, DeclEntry>,
    /// Global variable declarations. Keyed by `Sym` (string symbol)
    /// rather than `TypeNameSym` because globals don't live in the
    /// type-name namespace.
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
            decls: FxHashMap::default(),
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
