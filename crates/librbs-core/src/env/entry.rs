use crate::interner::TypeNameSym;

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

/// Marks an entry in `Environment::class_decls` as either a class or a
/// module. The variant must remain consistent across all decls inserted
/// under the same name; mixing is reported as a duplicate-declaration
/// error during insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassLikeEntry {
    Class,
    Module,
}

/// Entry in `Environment::class_alias_decls`. The resolver reads
/// `(old_name, context)` to seed `TypeNameResolver::aliases`. The
/// `Class`/`Module` distinction upstream RBS makes between
/// `ClassAlias` and `ModuleAlias` is not consulted here, so a single
/// struct covers both.
#[derive(Debug)]
pub struct ClassAliasEntry {
    pub old_name: TypeNameSym,
    pub context: Context,
}
