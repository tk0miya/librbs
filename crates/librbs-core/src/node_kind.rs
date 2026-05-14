//! Closed-enum classifications of `ruby_rbs::node::Node`.
//!
//! Each `*Kind::from_node` performs the *only* match in librbs-core that
//! enumerates every `Node` variant without a wildcard arm. A future
//! `ruby-rbs` bump that adds a new variant therefore fails to build at
//! the matching `from_node` instead of being silently swallowed by a
//! `_ => {}` arm in one of the resolver's walks. Once classified, the
//! walks dispatch on the closed kind enum, which is exhaustive by
//! construction — no further wildcards needed.
//!
//! Lifetime convention: `'a` is the parser's lifetime (carried by every
//! `*Node<'a>`); `'b` is the borrow lifetime of the source `&Node<'a>`.
//! The kind enums hold `&'b *Node<'a>` so callers can keep using the
//! same references they already had.

use ruby_rbs::node::{
    AliasNode, AliasTypeNode, AnyTypeNode, AttrAccessorNode, AttrReaderNode, AttrWriterNode,
    BlockTypeNode, BoolTypeNode, BottomTypeNode, ClassAliasNode, ClassInstanceTypeNode,
    ClassInstanceVariableNode, ClassNode, ClassSingletonTypeNode, ClassTypeNode, ClassVariableNode,
    ConstantNode, ExtendNode, FunctionTypeNode, GlobalNode, IncludeNode, InstanceTypeNode,
    InstanceVariableNode, InterfaceNode, InterfaceTypeNode, IntersectionTypeNode, LiteralTypeNode,
    MethodDefinitionNode, ModuleAliasNode, ModuleNode, NilTypeNode, Node, OptionalTypeNode,
    PrependNode, PrivateNode, ProcTypeNode, PublicNode, RecordFieldTypeNode, RecordTypeNode,
    SelfTypeNode, TopTypeNode, TupleTypeNode, TypeAliasNode, UnionTypeNode,
    UntypedFunctionTypeNode, UseSingleClauseNode, UseWildcardClauseNode, VariableTypeNode,
    VoidTypeNode,
};

// ---------- DeclKind ----------

/// Top-level / nested declaration kinds — `RBS::AST::Declarations::*`.
/// Membership is exposed through [`crate::env::insert::is_decl_node`],
/// which delegates to [`DeclKind::from_node`].
#[allow(dead_code)] // each variant's payload is dispatched through pattern matches; some no-op arms bind via `_`.
pub enum DeclKind<'b, 'a> {
    Class(&'b ClassNode<'a>),
    Module(&'b ModuleNode<'a>),
    Interface(&'b InterfaceNode<'a>),
    TypeAlias(&'b TypeAliasNode<'a>),
    Constant(&'b ConstantNode<'a>),
    Global(&'b GlobalNode<'a>),
    ClassAlias(&'b ClassAliasNode<'a>),
    ModuleAlias(&'b ModuleAliasNode<'a>),
}

impl<'b, 'a> DeclKind<'b, 'a> {
    pub fn from_node(n: &'b Node<'a>) -> Option<Self> {
        classify_node(n).into_decl()
    }
}

// ---------- MemberKind ----------

/// Class / module / interface body member kinds —
/// `RBS::AST::Members::*`. The three trailing variants (`Public`,
/// `Private`, `Alias`) carry no type-name occurrences and are dispatched
/// as no-ops by the walk.
#[allow(dead_code)]
pub enum MemberKind<'b, 'a> {
    MethodDefinition(&'b MethodDefinitionNode<'a>),
    AttrAccessor(&'b AttrAccessorNode<'a>),
    AttrReader(&'b AttrReaderNode<'a>),
    AttrWriter(&'b AttrWriterNode<'a>),
    InstanceVariable(&'b InstanceVariableNode<'a>),
    ClassInstanceVariable(&'b ClassInstanceVariableNode<'a>),
    ClassVariable(&'b ClassVariableNode<'a>),
    Include(&'b IncludeNode<'a>),
    Extend(&'b ExtendNode<'a>),
    Prepend(&'b PrependNode<'a>),
    Public(&'b PublicNode<'a>),
    Private(&'b PrivateNode<'a>),
    Alias(&'b AliasNode<'a>),
}

impl<'b, 'a> MemberKind<'b, 'a> {
    pub fn from_node(n: &'b Node<'a>) -> Option<Self> {
        classify_node(n).into_member()
    }
}

// ---------- TypeKind ----------

/// Type-expression kinds — `RBS::Types::*`. Mirrors the positive arms of
/// `walk_type`.
#[allow(dead_code)]
pub enum TypeKind<'b, 'a> {
    ClassInstance(&'b ClassInstanceTypeNode<'a>),
    Interface(&'b InterfaceTypeNode<'a>),
    Alias(&'b AliasTypeNode<'a>),
    ClassSingleton(&'b ClassSingletonTypeNode<'a>),
    Tuple(&'b TupleTypeNode<'a>),
    Union(&'b UnionTypeNode<'a>),
    Intersection(&'b IntersectionTypeNode<'a>),
    Record(&'b RecordTypeNode<'a>),
    Optional(&'b OptionalTypeNode<'a>),
    Proc(&'b ProcTypeNode<'a>),
    Function(&'b FunctionTypeNode<'a>),
    UntypedFunction(&'b UntypedFunctionTypeNode<'a>),
    Literal(&'b LiteralTypeNode<'a>),
    Variable(&'b VariableTypeNode<'a>),
    Bool(&'b BoolTypeNode<'a>),
    Void(&'b VoidTypeNode<'a>),
    Any(&'b AnyTypeNode<'a>),
    Nil(&'b NilTypeNode<'a>),
    Top(&'b TopTypeNode<'a>),
    Bottom(&'b BottomTypeNode<'a>),
    SelfType(&'b SelfTypeNode<'a>),
    Instance(&'b InstanceTypeNode<'a>),
    Class(&'b ClassTypeNode<'a>),
    Block(&'b BlockTypeNode<'a>),
    RecordField(&'b RecordFieldTypeNode<'a>),
}

impl<'b, 'a> TypeKind<'b, 'a> {
    pub fn from_node(n: &'b Node<'a>) -> Option<Self> {
        classify_node(n).into_type()
    }
}

// ---------- UseClauseKind ----------

/// `# use ...` directive clause kinds. The C parser only emits these
/// two as children of a `UseNode`.
#[allow(dead_code)]
pub enum UseClauseKind<'b, 'a> {
    Single(&'b UseSingleClauseNode<'a>),
    Wildcard(&'b UseWildcardClauseNode<'a>),
}

impl<'b, 'a> UseClauseKind<'b, 'a> {
    pub fn from_node(n: &'b Node<'a>) -> Option<Self> {
        classify_node(n).into_use_clause()
    }
}

// ---------- Central classifier ----------
//
// `classify_node` is the *only* function in this crate that matches
// every `Node` variant without a wildcard. A future `ruby-rbs` bump
// that adds a new variant fails to build here. The `*Kind::from_node`
// wrappers above filter the union result into one category.

#[allow(dead_code)]
enum NodeKind<'b, 'a> {
    Decl(DeclKind<'b, 'a>),
    Member(MemberKind<'b, 'a>),
    Type(TypeKind<'b, 'a>),
    UseClause(UseClauseKind<'b, 'a>),
    /// Structural / sub-component / literal nodes that none of the
    /// resolver's walks dispatch on directly. Listing them here keeps
    /// the central match exhaustive without forcing the dispatchers to
    /// know about them individually.
    Other,
}

impl<'b, 'a> NodeKind<'b, 'a> {
    fn into_decl(self) -> Option<DeclKind<'b, 'a>> {
        match self {
            NodeKind::Decl(d) => Some(d),
            _ => None,
        }
    }
    fn into_member(self) -> Option<MemberKind<'b, 'a>> {
        match self {
            NodeKind::Member(m) => Some(m),
            _ => None,
        }
    }
    fn into_type(self) -> Option<TypeKind<'b, 'a>> {
        match self {
            NodeKind::Type(t) => Some(t),
            _ => None,
        }
    }
    fn into_use_clause(self) -> Option<UseClauseKind<'b, 'a>> {
        match self {
            NodeKind::UseClause(u) => Some(u),
            _ => None,
        }
    }
}

fn classify_node<'b, 'a>(n: &'b Node<'a>) -> NodeKind<'b, 'a> {
    match n {
        // --- Declarations ---
        Node::Class(c) => NodeKind::Decl(DeclKind::Class(c)),
        Node::Module(m) => NodeKind::Decl(DeclKind::Module(m)),
        Node::Interface(i) => NodeKind::Decl(DeclKind::Interface(i)),
        Node::TypeAlias(a) => NodeKind::Decl(DeclKind::TypeAlias(a)),
        Node::Constant(c) => NodeKind::Decl(DeclKind::Constant(c)),
        Node::Global(g) => NodeKind::Decl(DeclKind::Global(g)),
        Node::ClassAlias(a) => NodeKind::Decl(DeclKind::ClassAlias(a)),
        Node::ModuleAlias(a) => NodeKind::Decl(DeclKind::ModuleAlias(a)),

        // --- Members ---
        Node::MethodDefinition(m) => NodeKind::Member(MemberKind::MethodDefinition(m)),
        Node::AttrAccessor(a) => NodeKind::Member(MemberKind::AttrAccessor(a)),
        Node::AttrReader(a) => NodeKind::Member(MemberKind::AttrReader(a)),
        Node::AttrWriter(a) => NodeKind::Member(MemberKind::AttrWriter(a)),
        Node::InstanceVariable(v) => NodeKind::Member(MemberKind::InstanceVariable(v)),
        Node::ClassInstanceVariable(v) => NodeKind::Member(MemberKind::ClassInstanceVariable(v)),
        Node::ClassVariable(v) => NodeKind::Member(MemberKind::ClassVariable(v)),
        Node::Include(m) => NodeKind::Member(MemberKind::Include(m)),
        Node::Extend(m) => NodeKind::Member(MemberKind::Extend(m)),
        Node::Prepend(m) => NodeKind::Member(MemberKind::Prepend(m)),
        Node::Public(p) => NodeKind::Member(MemberKind::Public(p)),
        Node::Private(p) => NodeKind::Member(MemberKind::Private(p)),
        Node::Alias(a) => NodeKind::Member(MemberKind::Alias(a)),

        // --- Types ---
        Node::ClassInstanceType(t) => NodeKind::Type(TypeKind::ClassInstance(t)),
        Node::InterfaceType(t) => NodeKind::Type(TypeKind::Interface(t)),
        Node::AliasType(t) => NodeKind::Type(TypeKind::Alias(t)),
        Node::ClassSingletonType(t) => NodeKind::Type(TypeKind::ClassSingleton(t)),
        Node::TupleType(t) => NodeKind::Type(TypeKind::Tuple(t)),
        Node::UnionType(t) => NodeKind::Type(TypeKind::Union(t)),
        Node::IntersectionType(t) => NodeKind::Type(TypeKind::Intersection(t)),
        Node::RecordType(t) => NodeKind::Type(TypeKind::Record(t)),
        Node::OptionalType(t) => NodeKind::Type(TypeKind::Optional(t)),
        Node::ProcType(t) => NodeKind::Type(TypeKind::Proc(t)),
        Node::FunctionType(t) => NodeKind::Type(TypeKind::Function(t)),
        Node::UntypedFunctionType(t) => NodeKind::Type(TypeKind::UntypedFunction(t)),
        Node::LiteralType(t) => NodeKind::Type(TypeKind::Literal(t)),
        Node::VariableType(t) => NodeKind::Type(TypeKind::Variable(t)),
        Node::BoolType(t) => NodeKind::Type(TypeKind::Bool(t)),
        Node::VoidType(t) => NodeKind::Type(TypeKind::Void(t)),
        Node::AnyType(t) => NodeKind::Type(TypeKind::Any(t)),
        Node::NilType(t) => NodeKind::Type(TypeKind::Nil(t)),
        Node::TopType(t) => NodeKind::Type(TypeKind::Top(t)),
        Node::BottomType(t) => NodeKind::Type(TypeKind::Bottom(t)),
        Node::SelfType(t) => NodeKind::Type(TypeKind::SelfType(t)),
        Node::InstanceType(t) => NodeKind::Type(TypeKind::Instance(t)),
        Node::ClassType(t) => NodeKind::Type(TypeKind::Class(t)),
        Node::BlockType(t) => NodeKind::Type(TypeKind::Block(t)),
        Node::RecordFieldType(t) => NodeKind::Type(TypeKind::RecordField(t)),

        // --- Use clauses ---
        Node::UseSingleClause(c) => NodeKind::UseClause(UseClauseKind::Single(c)),
        Node::UseWildcardClause(c) => NodeKind::UseClause(UseClauseKind::Wildcard(c)),

        // --- Other: not directly dispatched by the resolver walks. ---
        //
        // Subgrouped so reviewers can verify each placement against
        // `vendor/rbs/config.yml`.

        // Literal *values* used as the `literal:` field of
        // `RBS::Types::Literal` (e.g. `type x = 1` → LiteralType {
        // literal: Integer(1) }). These are values, not type
        // expressions — the type wrapper is `LiteralType`, which lives
        // in `TypeKind` above. All four are flagged
        // `expose_to_ruby: false` in `config.yml`.
        Node::Bool(_)
        | Node::Integer(_)
        | Node::String(_)
        | Node::Symbol(_)

        // Sub-components of nodes that *are* dispatched. The walk
        // descends into these inline via the parent's typed accessors
        // (`walk_class` reads `c.super_class()`, `walk_method_definition`
        // reads `m.overloads()`, etc.), so they never reach
        // `classify_node` as a top-level dispatch.
        | Node::ClassSuper(_)            // `< Bar` of ClassNode
        | Node::ModuleSelf(_)            // `: Bar` of ModuleNode
        | Node::MethodDefinitionOverload(_) // one overload of a method def
        | Node::FunctionParam(_)         // one param of a FunctionType
        | Node::TypeParam(_)             // walked by walk_type_params
        | Node::TypeName(_)              // walked via intern_type_name_node
        | Node::Namespace(_)             // sub-part of TypeName
        | Node::Signature(_)             // root container; iterated, not dispatched

        // MethodType is walked by `walk_method_type` from inside
        // `walk_method_definition` — never reaches a top-level dispatch.
        | Node::MethodType(_)

        // Top-level `# use ...` directive node. `apply_use_directive`
        // takes a `UseNode` directly; the dispatcher matches against
        // its *clauses* (which become `UseClauseKind`), not the wrapper.
        | Node::Use(_)

        // Metadata attached to decls/members (`%a{...}` strings and
        // RDoc comment blocks). Carry no type references.
        | Node::Annotation(_)
        | Node::Comment(_)

        // `RBS::AST::Ruby::Annotations::*` — inline `# @rbs ...`
        // annotations parsed from `.rb` files by `RBS::InlineParser`.
        // librbs-core's `Source` only models `RBS::Source::RBS`
        // (.rbs files), so these never reach the resolver. If Ruby
        // source support is added later, they need their own walker —
        // several of them (NodeTypeAssertion, ColonMethodTypeAnnotation,
        // ParamTypeAnnotation, ReturnTypeAnnotation,
        // TypeApplicationAnnotation, MethodTypesAnnotation,
        // SplatParamTypeAnnotation, DoubleSplatParamTypeAnnotation,
        // BlockParamTypeAnnotation, InstanceVariableAnnotation) carry
        // type references that would need resolving.
        | Node::BlockParamTypeAnnotation(_)
        | Node::ClassAliasAnnotation(_)
        | Node::ColonMethodTypeAnnotation(_)
        | Node::DoubleSplatParamTypeAnnotation(_)
        | Node::InstanceVariableAnnotation(_)
        | Node::MethodTypesAnnotation(_)
        | Node::ModuleAliasAnnotation(_)
        | Node::NodeTypeAssertion(_)
        | Node::ParamTypeAnnotation(_)
        | Node::ReturnTypeAnnotation(_)
        | Node::SkipAnnotation(_)
        | Node::SplatParamTypeAnnotation(_)
        | Node::TypeApplicationAnnotation(_) => NodeKind::Other,
    }
}
