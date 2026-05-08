use crate::interner::{Sym, TypeNameSym};

/// A reference to a particular declaration node, identifying its source
/// file and the index within that file's declaration list (the index is
/// pre-order over nested declarations).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeclRef {
    pub source_index: u32,
    pub decl_index: u32,
}

/// `Resolver::context` equivalent: a chain of namespace nodes, leading from
/// the outermost (None) inward.
pub type Context = Vec<TypeNameSym>;

#[derive(Debug)]
pub struct ClassEntry {
    pub name: TypeNameSym,
    pub context_decls: Vec<(Context, DeclRef)>,
}

#[derive(Debug)]
pub struct ModuleEntry {
    pub name: TypeNameSym,
    pub context_decls: Vec<(Context, DeclRef)>,
}

/// Stores either a Class or a Module entry. The variant must remain
/// consistent across all decls inserted under the same name.
#[derive(Debug)]
pub enum ClassLikeEntry {
    Class(ClassEntry),
    Module(ModuleEntry),
}

impl ClassLikeEntry {
    pub fn name(&self) -> TypeNameSym {
        match self {
            ClassLikeEntry::Class(c) => c.name,
            ClassLikeEntry::Module(m) => m.name,
        }
    }

    pub fn is_class(&self) -> bool {
        matches!(self, ClassLikeEntry::Class(_))
    }

    pub fn is_module(&self) -> bool {
        matches!(self, ClassLikeEntry::Module(_))
    }

    pub fn push(&mut self, ctx: Context, decl: DeclRef) {
        match self {
            ClassLikeEntry::Class(c) => c.context_decls.push((ctx, decl)),
            ClassLikeEntry::Module(m) => m.context_decls.push((ctx, decl)),
        }
    }
}

#[derive(Debug)]
pub struct InterfaceEntry {
    pub name: TypeNameSym,
    pub context: Context,
    pub decl: DeclRef,
}

#[derive(Debug)]
pub struct TypeAliasEntry {
    pub name: TypeNameSym,
    pub context: Context,
    pub decl: DeclRef,
}

#[derive(Debug)]
pub struct ConstantEntry {
    pub name: TypeNameSym,
    pub context: Context,
    pub decl: DeclRef,
}

#[derive(Debug)]
pub struct GlobalEntry {
    pub name: Sym,
    pub context: Context,
    pub decl: DeclRef,
}

#[derive(Debug)]
pub struct ClassAliasEntry {
    pub name: TypeNameSym,
    pub old_name: TypeNameSym,
    pub context: Context,
    pub decl: DeclRef,
}

#[derive(Debug)]
pub struct ModuleAliasEntry {
    pub name: TypeNameSym,
    pub old_name: TypeNameSym,
    pub context: Context,
    pub decl: DeclRef,
}

#[derive(Debug)]
pub enum ClassAliasLikeEntry {
    Class(ClassAliasEntry),
    Module(ModuleAliasEntry),
}

impl ClassAliasLikeEntry {
    pub fn name(&self) -> TypeNameSym {
        match self {
            ClassAliasLikeEntry::Class(c) => c.name,
            ClassAliasLikeEntry::Module(m) => m.name,
        }
    }

    pub fn old_name(&self) -> TypeNameSym {
        match self {
            ClassAliasLikeEntry::Class(c) => c.old_name,
            ClassAliasLikeEntry::Module(m) => m.old_name,
        }
    }

    pub fn context(&self) -> &Context {
        match self {
            ClassAliasLikeEntry::Class(c) => &c.context,
            ClassAliasLikeEntry::Module(m) => &m.context,
        }
    }
}
