//! M3b resolution driver.
//!
//! Walks every parsed source's AST, calls `TypeNameResolver::resolve` for
//! every type-name occurrence, and stores the outcome in a [`Resolution`]
//! side-table. The walk mirrors `RBS::Environment#resolve_*` family from
//! `vendor/rbs/lib/rbs/environment.rb:577-980` line-by-line; transcribed
//! Ruby ranges appear as `// environment.rb:NNN-NNN` comments above each
//! Rust counterpart so the reviewer can verify the correspondence.
//!
//! Out of M3b scope:
//! - `rayon` parallelization. The driver's AST walk runs almost
//!   entirely against `&TypeNameInterner` — every `TypeNameNode`
//!   inside a declaration was pre-interned by
//!   `env::insert::insert_rbs_source`. The two remaining `&mut interner`
//!   sites in resolve are [`apply_use_directive`], which interns each
//!   `# use ...` clause target as it registers it in the per-source
//!   `UseMap`, and [`UseMap::resolve`], which constructs a rewritten
//!   absolute name when a relative name's head is mapped by `# use`.
//!   A follow-up that pre-interns every use-rewrite (see
//!   `docs/tasks/followups.md`) would close the second; once both
//!   close, per-source `par_iter` becomes a one-line change.
//! - Magnus / Ruby boundary work — that is M3c.
//!
//! `walk_*` functions are a one-for-one port of the Ruby `resolve_*`
//! family. Each Ruby `case decl when ...` branch becomes a Rust match arm
//! with a `// environment.rb:NNN-NNN` comment.

use rustc_hash::FxHashSet;

use ruby_rbs::node::{
    AliasTypeNode, AttrAccessorNode, AttrReaderNode, AttrWriterNode, BlockTypeNode,
    ClassInstanceTypeNode, ClassNode, ClassSingletonTypeNode, FunctionTypeNode, InterfaceNode,
    InterfaceTypeNode, MethodDefinitionNode, MethodTypeNode, ModuleNode, Node, ProcTypeNode,
    RecordTypeNode, TypeNameNode, TypeParamNode, UseNode, UseSingleClauseNode,
    UseWildcardClauseNode,
};

use crate::env::Environment;
use crate::env::entry::{Context, DeclRef};
use crate::env::insert::{find_type_name_node, intern_type_name_node, is_decl_node};
use crate::env::resolution::{Resolution, ResolvedRef};
use crate::env::use_map::{Table, UseMap};
use crate::interner::{FrozenInterner, Sym, TypeNameInterner, TypeNameSym};
use crate::resolver::TypeNameResolver;
use crate::source::Source;

/// `RBS::Environment#resolve_type_names`. Builds the resolver and the
/// global `UseMap::Table`, then drives every source through
/// [`resolve_source`]. When `only` is `Some`, declarations whose
/// entry-name isn't in the set are skipped — their type-name
/// occurrences never enter the [`Resolution`] table, and downstream
/// materialization falls back to the AST's original names (matching
/// upstream's "decl is returned as-is" semantics for
/// `resolve_type_names(only:)`).
///
/// Sequential for the reasons outlined in the module header. Returns a
/// merged [`Resolution`].
//
// environment.rb:522-560
pub fn resolve(env: &mut Environment, only: Option<&FxHashSet<TypeNameSym>>) -> Resolution {
    let mut resolver = TypeNameResolver::build(env);
    let mut table = Table::new();
    table.populate_from(env);
    table.compute_children(&env.interner);

    let mut resolution = Resolution::new();
    let n = env.sources.len();
    for idx in 0..n {
        // SAFETY-equivalent split: `sources[idx]` is borrowed immutably as
        // a *raw pointer for the AST traversal while we mutate
        // `env.interner` and `env.resolution` via &mut Environment. The
        // raw pointer is only ever turned back into a shared reference
        // and never aliases a mutable borrow because the AST traversal
        // never touches `env.sources`.
        let src_ptr: *const Source = &env.sources[idx];
        // The cast is safe because `Source` is heap-stable (it owns a
        // `Box<str>` for content, and the parsed `SignatureNode` borrows
        // from that box). We never resize `env.sources` during resolve.
        let src: &Source = unsafe { &*src_ptr };
        resolve_source(
            idx as u32,
            src,
            env,
            &mut resolver,
            &table,
            &mut resolution,
            only,
        );
    }
    resolution
}

/// One source's worth of resolution. Builds a per-source [`UseMap`],
/// honors the `# resolve-type-names: false` magic comment, then runs
/// the per-decl walk.
//
// environment.rb:500-541
fn resolve_source(
    source_index: u32,
    source: &Source,
    env: &mut Environment,
    resolver: &mut TypeNameResolver,
    table: &Table,
    resolution: &mut Resolution,
    only: Option<&FxHashSet<TypeNameSym>>,
) {
    // environment.rb:534 — magic-comment short-circuit. When the source
    // begins with `# resolve-type-names: false`, the entire source is
    // returned as-is and no resolution table entries are written.
    if is_type_name_resolution_disabled(&source.buffer.content) {
        return;
    }

    // environment.rb:501-509 — UseMap construction. We walk every
    // top-level `UseNode` and translate its clauses through `add_single`
    // / `add_wildcard`. Non-`UseNode` directives (e.g. ResolveTypeNames)
    // are not exposed by the C parser (the magic comment is parsed in
    // Ruby), so they don't appear here.
    let mut use_map = UseMap::new(table);
    for dir in source.parser.signature().directives().iter() {
        if let Node::Use(u) = &dir {
            apply_use_directive(u, &mut use_map, &mut env.interner);
        }
    }

    let mut ctx = WalkCtx {
        source_index,
        decl_counter: 0,
        current_decl_ref: None,
        env,
        resolver,
        use_map,
        resolution,
    };

    // environment.rb:511-517 — top-level decl loop. Each decl runs with
    // `context: nil` (i.e. our empty `Vec`) and `prefix: Namespace.root`.
    // The empty context selects the absolute-root scope inside the
    // resolver, matching upstream's `nil`-cons-cell sentinel.
    let context: Context = Vec::new();
    for decl in source.parser.signature().declarations().iter() {
        if let Some(set) = only
            && !decl_matches_only(&decl, set, ctx.env.interner.frozen())
        {
            // Run the same recursion `walk_declaration` would have, but
            // only perform its counter side effect — keeping
            // `decl_counter` aligned with insert's `DeclRef::decl_index`
            // numbering even across skips, so M3e materialization can
            // look entries up by `DeclRef` without an off-by-N gap from
            // `only:` filtering.
            consume_decl_ref(&decl, &mut ctx.decl_counter);
            continue;
        }
        walk_declaration(&mut ctx, &decl, &context);
    }
}

/// Walk `node` as `walk_declaration` would, performing only its
/// counter side effect — one `decl_counter += 1` per
/// `is_decl_node` visit, in the same pre-order as
/// `env::insert::insert_decl`. The match arms intentionally mirror
/// `walk_class` / `walk_module`'s nested-decl recursion (gated by
/// `is_decl_node`) so that any future change to which AST shapes
/// contain nested decls is applied in both places.
///
/// Direct counter increment is equivalent to
/// `WalkCtx::next_decl_ref` because the latter's only side effect
/// is `decl_counter += 1` — the returned `DeclRef` is unused by the
/// driver. If `next_decl_ref` ever gains additional state, this
/// helper must be updated alongside it.
///
/// Caller assumes `node` itself is a decl node; no `is_decl_node`
/// check on the root.
fn consume_decl_ref(node: &Node<'_>, counter: &mut u32) {
    *counter += 1;
    match node {
        Node::Class(c) => {
            for member in c.members().iter() {
                if is_decl_node(&member) {
                    consume_decl_ref(&member, counter);
                }
            }
        }
        Node::Module(m) => {
            for member in m.members().iter() {
                if is_decl_node(&member) {
                    consume_decl_ref(&member, counter);
                }
            }
        }
        // Interface / TypeAlias / Constant / Global / ClassAlias /
        // ModuleAlias have no nested decls — `insert_decl` issues a
        // single DeclRef and returns.
        _ => {}
    }
}

/// Compute the absolute name a top-level declaration would register
/// under, then test it against the `only:` filter. Anchor against the
/// absolute root (mirrors `prefix: Namespace.root` in
/// `RBS::Environment#resolve_signature`, environment.rb:515) so the
/// comparison key matches exactly the entry-name M2 inserted into the
/// `*_decls` tables.
fn decl_matches_only(
    decl: &Node<'_>,
    only: &FxHashSet<TypeNameSym>,
    interner: FrozenInterner<'_>,
) -> bool {
    let name_node = match decl {
        Node::Class(c) => c.name(),
        Node::Module(m) => m.name(),
        Node::Interface(i) => i.name(),
        Node::TypeAlias(a) => a.name(),
        Node::Constant(c) => c.name(),
        Node::ClassAlias(a) => a.new_name(),
        Node::ModuleAlias(a) => a.new_name(),
        // Globals key by `Sym`, never by `TypeNameSym` — they can never
        // be selected by an `only:` set of `TypeName`s. Skip them.
        Node::Global(_) => return false,
        _ => return false,
    };
    let Some(inner) = find_type_name_node(interner, &name_node) else {
        return false;
    };
    let root = interner.namespaces().root_absolute();
    let Some(full) = interner.with_prefix(root, inner) else {
        return false;
    };
    only.contains(&full)
}

/// Detects the `# resolve-type-names: false` magic comment at the very
/// start of a source. Mirrors upstream's anchor-at-`\A` regex
///
/// ```text
/// /\A#\s*resolve-type-names\s*:\s+(true|false)$/
/// ```
///
/// from `vendor/rbs/lib/rbs/parser_aux.rb:51`. We deliberately do not
/// pull in a regex engine for one call site; the chain of
/// `strip_prefix` / `trim_start_matches` matches each fragment of the
/// upstream pattern position-by-position, with the same anchoring
/// semantics (only the absolute first line is considered).
fn is_type_name_resolution_disabled(content: &str) -> bool {
    let first_line = content.lines().next().unwrap_or("");
    // \A#
    let Some(s) = first_line.strip_prefix('#') else {
        return false;
    };
    // \s*
    let s = s.trim_start_matches([' ', '\t']);
    // resolve-type-names
    let Some(s) = s.strip_prefix("resolve-type-names") else {
        return false;
    };
    // \s*
    let s = s.trim_start_matches([' ', '\t']);
    // :
    let Some(s) = s.strip_prefix(':') else {
        return false;
    };
    // \s+ — at least one whitespace required; reject `:false` form.
    let after_ws = s.trim_start_matches([' ', '\t']);
    if after_ws.len() == s.len() {
        return false;
    }
    // (true|false)$ — we only short-circuit on `false`; `true` is the
    // default and behaves the same as no directive.
    after_ws == "false"
}

/// All the per-source mutable state the walk needs. Bundled into one
/// struct so the recursive walk function signatures don't accumulate
/// half a dozen arguments.
struct WalkCtx<'env, 'tab> {
    source_index: u32,
    /// Incremented for every visited declaration (top-level or nested).
    /// Matches the sequencing in `env::insert::insert_rbs_source`.
    decl_counter: u32,
    /// The [`DeclRef`] currently being walked. Set by
    /// [`walk_declaration`] on entry and restored on exit so nested
    /// decls push to their own resolution slice rather than the
    /// outer's. `None` only at top level before the first decl, where
    /// no [`record_type_name`] call should ever fire (top-level walks
    /// dispatch into a decl arm immediately).
    current_decl_ref: Option<DeclRef>,
    env: &'env mut Environment,
    resolver: &'env mut TypeNameResolver,
    use_map: UseMap<'tab>,
    resolution: &'env mut Resolution,
}

impl WalkCtx<'_, '_> {
    fn next_decl_ref(&mut self) -> DeclRef {
        let decl_index = self.decl_counter;
        self.decl_counter += 1;
        DeclRef {
            source_index: self.source_index,
            decl_index,
        }
    }
}

/// `absolute_type_name` (environment.rb:982-985):
///
/// ```ruby
/// def absolute_type_name(resolver, map, type_name, context:)
///   type_name = map.resolve(type_name) if map
///   resolver.resolve(type_name, context: context) || type_name
/// end
/// ```
fn record_type_name(ctx: &mut WalkCtx<'_, '_>, raw: TypeNameSym, context: &Context) {
    let mapped = ctx.use_map.resolve(raw, &mut ctx.env.interner);
    let resolved = ctx
        .resolver
        .resolve(mapped, context, ctx.env.interner.frozen());
    let entry = match resolved {
        Some(sym) => ResolvedRef::Resolved(sym),
        None => ResolvedRef::Unresolved(mapped),
    };
    let decl_ref = ctx
        .current_decl_ref
        .expect("record_type_name called outside any decl walk — driver invariant violation");
    ctx.resolution.record(decl_ref, entry);
}

/// Apply one parsed `# use ...` directive to the [`UseMap`]. Mirrors
/// `UseMap#build_map` (`vendor/rbs/lib/rbs/environment/use_map.rb`),
/// dispatched on the clause node type from the C parser.
fn apply_use_directive(u: &UseNode<'_>, map: &mut UseMap<'_>, interner: &mut TypeNameInterner) {
    for clause in u.clauses().iter() {
        match clause {
            Node::UseSingleClause(c) => apply_use_single_clause(&c, map, interner),
            Node::UseWildcardClause(c) => apply_use_wildcard_clause(&c, map, interner),
            _ => {}
        }
    }
}

fn apply_use_single_clause(
    c: &UseSingleClauseNode<'_>,
    map: &mut UseMap<'_>,
    interner: &mut TypeNameInterner,
) {
    // The directive's `type_name` is the absolute target. Intern it as
    // an absolute name (the parsed namespace node already carries the
    // `absolute` flag for `::A::B`-style paths; the C parser disallows
    // relative use targets).
    let tn = intern_type_name_node(interner, &c.type_name());
    let new_name = c
        .new_name()
        .map(|sym| interner.symbols.intern(sym.as_str()));
    map.add_single(tn, new_name, interner);
}

fn apply_use_wildcard_clause(
    c: &UseWildcardClauseNode<'_>,
    map: &mut UseMap<'_>,
    interner: &mut TypeNameInterner,
) {
    let ns_node = c.namespace();
    let absolute = ns_node.absolute();
    let mut path: Vec<Sym> = Vec::new();
    for seg in ns_node.path().iter() {
        if let Node::Symbol(sym) = seg {
            path.push(interner.symbols.intern(sym.as_str()));
        }
    }
    let ns = interner.namespaces.intern(&path, absolute);
    map.add_wildcard(ns, interner);
}

// ----- declaration walk -----

/// `resolve_declaration` — environment.rb:577-711. Each Ruby `when`
/// branch maps to one Rust match arm.
fn walk_declaration(ctx: &mut WalkCtx<'_, '_>, decl: &Node<'_>, context: &Context) {
    let decl_ref = ctx.next_decl_ref();
    let prev_decl_ref = ctx.current_decl_ref.replace(decl_ref);
    match decl {
        // environment.rb:578-587 — Global. type only; no rename-to-prefix.
        Node::Global(g) => {
            let empty_ctx: Context = Vec::new();
            walk_type(ctx, &g.type_(), &empty_ctx);
        }
        // environment.rb:590-624 — Class.
        Node::Class(c) => walk_class(ctx, c, context),
        // environment.rb:626-660 — Module.
        Node::Module(m) => walk_module(ctx, m, context),
        // environment.rb:662-672 — Interface.
        Node::Interface(i) => walk_interface(ctx, i, context),
        // environment.rb:674-682 — TypeAlias.
        Node::TypeAlias(a) => {
            walk_type_params(ctx, &a.type_params(), context);
            walk_type(ctx, &a.type_(), context);
        }
        // environment.rb:684-691 — Constant.
        Node::Constant(c) => walk_type(ctx, &c.type_(), context),
        // environment.rb:693-700 — ClassAlias.
        Node::ClassAlias(a) => {
            // new_name carries no resolvable reference (it is the LHS of
            // the alias definition); only old_name is recorded.
            let old = find_type_name_node(ctx.env.interner.frozen(), &a.old_name())
                .expect("alias old_name pre-interned by insert");
            record_type_name(ctx, old, context);
        }
        // environment.rb:702-709 — ModuleAlias.
        Node::ModuleAlias(a) => {
            let old = find_type_name_node(ctx.env.interner.frozen(), &a.old_name())
                .expect("alias old_name pre-interned by insert");
            record_type_name(ctx, old, context);
        }
        _ => {}
    }
    ctx.current_decl_ref = prev_decl_ref;
}

fn walk_class(ctx: &mut WalkCtx<'_, '_>, c: &ClassNode<'_>, outer_context: &Context) {
    // environment.rb:591-594 — outer context, then push the inner.
    let full_name = full_decl_name(ctx.env.interner.frozen(), &c.name(), outer_context);
    let mut inner_context = outer_context.clone();
    inner_context.push(full_name);

    // environment.rb:597 — type_params.
    walk_type_params(ctx, &c.type_params(), &inner_context);

    // environment.rb:598-604 — super_class against the OUTER context.
    if let Some(sc) = c.super_class() {
        let super_tn = find_type_name_node(ctx.env.interner.frozen(), &sc.name())
            .expect("super class name pre-interned by insert");
        record_type_name(ctx, super_tn, outer_context);
        for arg in sc.args().iter() {
            walk_type(ctx, &arg, outer_context);
        }
    }

    // environment.rb:605-620 — members or nested decls, all under inner.
    for member in c.members().iter() {
        walk_class_or_module_child(ctx, &member, &inner_context);
    }
}

fn walk_module(ctx: &mut WalkCtx<'_, '_>, m: &ModuleNode<'_>, outer_context: &Context) {
    // environment.rb:627-632.
    let full_name = full_decl_name(ctx.env.interner.frozen(), &m.name(), outer_context);
    let mut inner_context = outer_context.clone();
    inner_context.push(full_name);

    walk_type_params(ctx, &m.type_params(), &inner_context);

    // environment.rb:634-640 — module self-types use the INNER context.
    for st in m.self_types().iter() {
        if let Node::ModuleSelf(ms) = &st {
            let tn = find_type_name_node(ctx.env.interner.frozen(), &ms.name())
                .expect("module self-type name pre-interned by insert");
            record_type_name(ctx, tn, &inner_context);
            for arg in ms.args().iter() {
                walk_type(ctx, &arg, &inner_context);
            }
        }
    }

    for member in m.members().iter() {
        walk_class_or_module_child(ctx, &member, &inner_context);
    }
}

fn walk_interface(ctx: &mut WalkCtx<'_, '_>, i: &InterfaceNode<'_>, context: &Context) {
    // environment.rb:663-672 — interface body keeps the same context.
    walk_type_params(ctx, &i.type_params(), context);
    for member in i.members().iter() {
        walk_member(ctx, &member, context);
    }
}

/// environment.rb:606-619. Class/module bodies dispatch on whether a
/// child is a member (`AST::Members::Base`) or another declaration.
fn walk_class_or_module_child(ctx: &mut WalkCtx<'_, '_>, node: &Node<'_>, context: &Context) {
    if is_decl_node(node) {
        walk_declaration(ctx, node, context);
    } else {
        walk_member(ctx, node, context);
    }
}

/// `decl.name.with_prefix(prefix)` for nested declarations. Walks the
/// outer context (innermost first) and prepends each segment to the
/// declaration's own namespace path.
fn full_decl_name(
    interner: FrozenInterner<'_>,
    decl_name: &TypeNameNode<'_>,
    outer_context: &Context,
) -> TypeNameSym {
    // The unprefixed inner name as written at the source.
    let inner = find_type_name_node(interner, decl_name).expect("decl name pre-interned by insert");
    // The outer context's innermost namespace, or the absolute root.
    let prefix = match outer_context.last() {
        Some(&parent) => interner
            .to_namespace(parent)
            .expect("parent namespace pre-interned by insert"),
        None => interner.namespaces().root_absolute(),
    };
    interner
        .with_prefix(prefix, inner)
        .expect("full decl name pre-interned by insert")
}

// ----- member walk -----

/// `resolve_member` — environment.rb:868-966. Each branch corresponds
/// one-for-one with the upstream `case member when ...`.
fn walk_member(ctx: &mut WalkCtx<'_, '_>, member: &Node<'_>, context: &Context) {
    match member {
        // environment.rb:870-884 — MethodDefinition.
        Node::MethodDefinition(m) => walk_method_definition(ctx, m, context),
        // environment.rb:885-895 — AttrAccessor.
        Node::AttrAccessor(a) => walk_attr_accessor(ctx, a, context),
        // environment.rb:896-906 — AttrReader.
        Node::AttrReader(a) => walk_attr_reader(ctx, a, context),
        // environment.rb:907-917 — AttrWriter.
        Node::AttrWriter(a) => walk_attr_writer(ctx, a, context),
        // environment.rb:918-924 — InstanceVariable.
        Node::InstanceVariable(v) => walk_type(ctx, &v.type_(), context),
        // environment.rb:925-931 — ClassInstanceVariable.
        Node::ClassInstanceVariable(v) => walk_type(ctx, &v.type_(), context),
        // environment.rb:932-938 — ClassVariable.
        Node::ClassVariable(v) => walk_type(ctx, &v.type_(), context),
        // environment.rb:939-946 — Include.
        Node::Include(m) => walk_mixin(ctx, &m.name(), m.args().iter(), context),
        // environment.rb:947-954 — Extend.
        Node::Extend(m) => walk_mixin(ctx, &m.name(), m.args().iter(), context),
        // environment.rb:955-962 — Prepend.
        Node::Prepend(m) => walk_mixin(ctx, &m.name(), m.args().iter(), context),
        // environment.rb:963-965 — fallthrough: Public, Private, Alias
        // carry no type-name occurrences and are returned unchanged.
        _ => {}
    }
}

fn walk_method_definition(
    ctx: &mut WalkCtx<'_, '_>,
    m: &MethodDefinitionNode<'_>,
    context: &Context,
) {
    // environment.rb:874-878 — overloads in source order.
    for overload in m.overloads().iter() {
        if let Node::MethodDefinitionOverload(o) = &overload
            && let Node::MethodType(mt) = o.method_type()
        {
            walk_method_type(ctx, &mt, context);
        }
    }
}

fn walk_attr_accessor(ctx: &mut WalkCtx<'_, '_>, a: &AttrAccessorNode<'_>, context: &Context) {
    walk_type(ctx, &a.type_(), context);
}

fn walk_attr_reader(ctx: &mut WalkCtx<'_, '_>, a: &AttrReaderNode<'_>, context: &Context) {
    walk_type(ctx, &a.type_(), context);
}

fn walk_attr_writer(ctx: &mut WalkCtx<'_, '_>, a: &AttrWriterNode<'_>, context: &Context) {
    walk_type(ctx, &a.type_(), context);
}

fn walk_mixin<'a>(
    ctx: &mut WalkCtx<'_, '_>,
    name: &TypeNameNode<'a>,
    args: impl Iterator<Item = Node<'a>>,
    context: &Context,
) {
    let tn = find_type_name_node(ctx.env.interner.frozen(), name)
        .expect("mixin name pre-interned by insert");
    record_type_name(ctx, tn, context);
    for arg in args {
        walk_type(ctx, &arg, context);
    }
}

// ----- method type / type params -----

/// `resolve_method_type` — environment.rb:968-974.
fn walk_method_type(ctx: &mut WalkCtx<'_, '_>, mt: &MethodTypeNode<'_>, context: &Context) {
    walk_type_params(ctx, &mt.type_params(), context);
    walk_type(ctx, &mt.type_(), context);
    if let Some(block) = mt.block() {
        walk_block(ctx, &block, context);
    }
}

fn walk_block(ctx: &mut WalkCtx<'_, '_>, b: &BlockTypeNode<'_>, context: &Context) {
    walk_type(ctx, &b.type_(), context);
    if let Some(self_t) = b.self_type() {
        walk_type(ctx, &self_t, context);
    }
}

/// `resolve_type_params` — environment.rb:976-980, plus `TypeParam#map_type`
/// (vendor/rbs/lib/rbs/ast/type_param.rb) which walks `upper_bound`,
/// `lower_bound`, and `default_type`.
fn walk_type_params<'a>(
    ctx: &mut WalkCtx<'_, '_>,
    params: &ruby_rbs::node::NodeList<'a>,
    context: &Context,
) {
    for p in params.iter() {
        if let Node::TypeParam(tp) = &p {
            walk_type_param(ctx, tp, context);
        }
    }
}

fn walk_type_param(ctx: &mut WalkCtx<'_, '_>, tp: &TypeParamNode<'_>, context: &Context) {
    if let Some(ub) = tp.upper_bound() {
        walk_type(ctx, &ub, context);
    }
    if let Some(lb) = tp.lower_bound() {
        walk_type(ctx, &lb, context);
    }
    if let Some(dt) = tp.default_type() {
        walk_type(ctx, &dt, context);
    }
}

// ----- type walk -----

/// `absolute_type` / `Types::*#map_type_name` — vendor/rbs/lib/rbs/types.rb.
/// Each `RBS::Types::*` variant gets its own arm.
fn walk_type(ctx: &mut WalkCtx<'_, '_>, ty: &Node<'_>, context: &Context) {
    match ty {
        // types.rb: ClassInstance, Interface, Alias have name + args.
        Node::ClassInstanceType(t) => walk_class_instance_type(ctx, t, context),
        Node::InterfaceType(t) => walk_interface_type(ctx, t, context),
        Node::AliasType(t) => walk_alias_type(ctx, t, context),
        // types.rb: ClassSingleton has only name.
        Node::ClassSingletonType(t) => walk_class_singleton_type(ctx, t, context),
        // types.rb: Tuple, Union, Intersection, Record have child types.
        Node::TupleType(t) => {
            for el in t.types().iter() {
                walk_type(ctx, &el, context);
            }
        }
        Node::UnionType(t) => {
            for el in t.types().iter() {
                walk_type(ctx, &el, context);
            }
        }
        Node::IntersectionType(t) => {
            for el in t.types().iter() {
                walk_type(ctx, &el, context);
            }
        }
        Node::RecordType(t) => walk_record_type(ctx, t, context),
        // types.rb: Optional wraps a single type.
        Node::OptionalType(t) => walk_type(ctx, &t.type_(), context),
        // types.rb: Proc has a function and an optional block.
        Node::ProcType(t) => walk_proc_type(ctx, t, context),
        // types.rb: Function/UntypedFunction — nested under Proc/MethodType.
        Node::FunctionType(t) => walk_function_type(ctx, t, context),
        Node::UntypedFunctionType(t) => walk_type(ctx, &t.return_type(), context),
        // types.rb: Literal — child is an Integer/String/Symbol/Bool node;
        // none carry type names.
        Node::LiteralType(_) => {}
        // types.rb: Variable, Bases::*, Block — nothing to record.
        Node::VariableType(_) => {}
        Node::BoolType(_)
        | Node::VoidType(_)
        | Node::AnyType(_)
        | Node::NilType(_)
        | Node::TopType(_)
        | Node::BottomType(_)
        | Node::SelfType(_)
        | Node::InstanceType(_)
        | Node::ClassType(_) => {}
        Node::BlockType(b) => walk_block(ctx, b, context),
        // RecordFieldType is reached via walk_record_type; if seen here
        // we still walk its child type for safety.
        Node::RecordFieldType(f) => walk_type(ctx, &f.type_(), context),
        _ => {}
    }
}

fn walk_class_instance_type(
    ctx: &mut WalkCtx<'_, '_>,
    t: &ClassInstanceTypeNode<'_>,
    context: &Context,
) {
    let tn = find_type_name_node(ctx.env.interner.frozen(), &t.name())
        .expect("class-instance type name pre-interned by insert");
    record_type_name(ctx, tn, context);
    for arg in t.args().iter() {
        walk_type(ctx, &arg, context);
    }
}

fn walk_interface_type(ctx: &mut WalkCtx<'_, '_>, t: &InterfaceTypeNode<'_>, context: &Context) {
    let tn = find_type_name_node(ctx.env.interner.frozen(), &t.name())
        .expect("interface type name pre-interned by insert");
    record_type_name(ctx, tn, context);
    for arg in t.args().iter() {
        walk_type(ctx, &arg, context);
    }
}

fn walk_alias_type(ctx: &mut WalkCtx<'_, '_>, t: &AliasTypeNode<'_>, context: &Context) {
    let tn = find_type_name_node(ctx.env.interner.frozen(), &t.name())
        .expect("alias type name pre-interned by insert");
    record_type_name(ctx, tn, context);
    for arg in t.args().iter() {
        walk_type(ctx, &arg, context);
    }
}

fn walk_class_singleton_type(
    ctx: &mut WalkCtx<'_, '_>,
    t: &ClassSingletonTypeNode<'_>,
    context: &Context,
) {
    let tn = find_type_name_node(ctx.env.interner.frozen(), &t.name())
        .expect("class-singleton type name pre-interned by insert");
    record_type_name(ctx, tn, context);
}

fn walk_record_type(ctx: &mut WalkCtx<'_, '_>, t: &RecordTypeNode<'_>, context: &Context) {
    for (_, value) in t.all_fields().iter() {
        if let Node::RecordFieldType(f) = &value {
            walk_type(ctx, &f.type_(), context);
        }
    }
}

fn walk_proc_type(ctx: &mut WalkCtx<'_, '_>, t: &ProcTypeNode<'_>, context: &Context) {
    walk_type(ctx, &t.type_(), context);
    if let Some(b) = t.block() {
        walk_block(ctx, &b, context);
    }
    if let Some(st) = t.self_type() {
        walk_type(ctx, &st, context);
    }
}

fn walk_function_type(ctx: &mut WalkCtx<'_, '_>, t: &FunctionTypeNode<'_>, context: &Context) {
    for p in t.required_positionals().iter() {
        walk_function_param(ctx, &p, context);
    }
    for p in t.optional_positionals().iter() {
        walk_function_param(ctx, &p, context);
    }
    if let Some(rest) = t.rest_positionals() {
        walk_function_param(ctx, &rest, context);
    }
    for p in t.trailing_positionals().iter() {
        walk_function_param(ctx, &p, context);
    }
    for (_, value) in t.required_keywords().iter() {
        walk_function_param(ctx, &value, context);
    }
    for (_, value) in t.optional_keywords().iter() {
        walk_function_param(ctx, &value, context);
    }
    if let Some(rest) = t.rest_keywords() {
        walk_function_param(ctx, &rest, context);
    }
    walk_type(ctx, &t.return_type(), context);
}

fn walk_function_param(ctx: &mut WalkCtx<'_, '_>, p: &Node<'_>, context: &Context) {
    if let Node::FunctionParam(fp) = p {
        walk_type(ctx, &fp.type_(), context);
    } else {
        // Defensive fallback: a non-FunctionParam at this position would
        // indicate a parser change. Treat the node itself as a type so
        // we still record any nested type names.
        walk_type(ctx, p, context);
    }
}

// ----- DeclRef helpers (consumed by tests) -----

/// Look up a declaration in a source by its [`DeclRef`]. Mirrors the
/// pre-order indexing in `env::insert::insert_rbs_source` so that for
/// every entry inserted there, the same `DeclRef` resolves back to the
/// same node here. Used by the M2 follow-up "DeclRef indexing
/// consistency between insert and lookup" round-trip test.
pub fn lookup_decl<'a>(source: &'a Source, decl_ref: DeclRef) -> Option<Node<'a>> {
    let target = decl_ref.decl_index;
    let mut counter: u32 = 0;
    for decl in source.parser.signature().declarations().iter() {
        if let Some(found) = walk_decl_index(&decl, &mut counter, target) {
            return Some(found);
        }
    }
    None
}

fn walk_decl_index<'a>(node: &Node<'a>, counter: &mut u32, target: u32) -> Option<Node<'a>> {
    if !is_decl_node(node) {
        return None;
    }
    let idx = *counter;
    *counter += 1;
    if idx == target {
        return Some(clone_node(node));
    }
    match node {
        Node::Class(c) => {
            for member in c.members().iter() {
                if is_decl_node(&member)
                    && let Some(hit) = walk_decl_index(&member, counter, target)
                {
                    return Some(hit);
                }
            }
        }
        Node::Module(m) => {
            for member in m.members().iter() {
                if is_decl_node(&member)
                    && let Some(hit) = walk_decl_index(&member, counter, target)
                {
                    return Some(hit);
                }
            }
        }
        _ => {}
    }
    None
}

/// Re-construct the same `Node<'a>` variant by destructuring and
/// rebuilding through `Node::*` field accessors. This avoids relying on
/// `Node: Clone` (the generated enum is not `Clone`).
fn clone_node<'a>(node: &Node<'a>) -> Node<'a> {
    // The bound fields inside `Node::*` are wrapper structs that hold a
    // `*mut rbs_node_t` plus a parser pointer; calling `as_node` on each
    // returns a fresh `Node` with the same inner pointers, which is the
    // cheapest way to round-trip.
    match node {
        Node::Class(c) => c.clone_node(),
        Node::Module(m) => m.clone_node(),
        Node::Interface(i) => i.clone_node(),
        Node::TypeAlias(t) => t.clone_node(),
        Node::Constant(c) => c.clone_node(),
        Node::Global(g) => g.clone_node(),
        Node::ClassAlias(a) => a.clone_node(),
        Node::ModuleAlias(a) => a.clone_node(),
        _ => unreachable!("clone_node only used for declaration nodes"),
    }
}

trait CloneNode<'a> {
    fn clone_node(&self) -> Node<'a>;
}

macro_rules! impl_clone_via_as_node {
    ($($t:ident),* $(,)?) => {
        $(
            impl<'a> CloneNode<'a> for ruby_rbs::node::$t<'a> {
                fn clone_node(&self) -> Node<'a> {
                    // The wrapper struct holds raw pointers plus a
                    // `PhantomData`; bytewise duplication is sound. We
                    // immediately consume the duplicate via `as_node`,
                    // which never mutates the underlying parser data.
                    let copy: ruby_rbs::node::$t<'a> =
                        unsafe { std::ptr::read(self as *const ruby_rbs::node::$t<'a>) };
                    copy.as_node()
                }
            }
        )*
    };
}

impl_clone_via_as_node!(
    ClassNode,
    ModuleNode,
    InterfaceNode,
    TypeAliasNode,
    ConstantNode,
    GlobalNode,
    ClassAliasNode,
    ModuleAliasNode,
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::ManagedParser;

    fn parse(src: &str) -> ManagedParser {
        ManagedParser::parse(src.to_string()).unwrap()
    }

    #[test]
    fn consume_decl_ref_advances_counter_per_pre_order_decl() {
        // The driver's `decl_counter` must advance by the same amount
        // `env::insert::insert_decl` would consume for the same
        // declaration subtree, otherwise `only:` skipping desyncs the
        // counter against insert's `DeclRef::decl_index` (M3e
        // materialization will look entries up by that index).
        //
        // Hand-counted expectations:
        // - leaf `class A end`                      → 1 (just A)
        // - `class A; class B end end`              → 2 (A, B)
        // - `class A; class B; class C end end end` → 3 (A, B, C)
        // - `module M; class A end; class B end end`→ 3 (M, A, B)
        // - `class A; def foo: -> void end`         → 1 (member is not a decl)
        // - `interface _Each end` (no nesting allowed) → 1
        let cases: &[(&str, u32)] = &[
            ("class A end\n", 1),
            ("class A\n  class B end\nend\n", 2),
            ("class A\n  class B\n    class C end\n  end\nend\n", 3),
            ("module M\n  class A end\n  class B end\nend\n", 3),
            ("class A\n  def foo: () -> void\nend\n", 1),
            ("interface _Each end\n", 1),
        ];
        for (src, expected) in cases {
            let parser = parse(src);
            let decl = parser.signature().declarations().iter().next().unwrap();
            let mut counter: u32 = 0;
            consume_decl_ref(&decl, &mut counter);
            assert_eq!(counter, *expected, "subtree count for {src:?}");
        }
    }

    #[test]
    fn detects_resolve_type_names_false_at_start() {
        assert!(is_type_name_resolution_disabled(
            "# resolve-type-names: false\n"
        ));
        assert!(is_type_name_resolution_disabled(
            "#  resolve-type-names :    false\n"
        ));
        // No newline at end of input — the directive on the only line
        // still counts.
        assert!(is_type_name_resolution_disabled(
            "# resolve-type-names: false"
        ));
    }

    #[test]
    fn ignores_resolve_type_names_true() {
        assert!(!is_type_name_resolution_disabled(
            "# resolve-type-names: true\n"
        ));
    }

    #[test]
    fn ignores_directive_not_at_start() {
        let src = "class Foo end\n# resolve-type-names: false\n";
        assert!(!is_type_name_resolution_disabled(src));
    }

    #[test]
    fn ignores_unrelated_comment() {
        assert!(!is_type_name_resolution_disabled("# coding: utf-8\n"));
    }

    #[test]
    fn requires_whitespace_between_colon_and_value() {
        // Upstream regex is `\s+` after the colon — `:false` without a
        // space must not match. This is stricter than the previous
        // implementation but matches `parser_aux.rb:51`.
        assert!(!is_type_name_resolution_disabled(
            "#resolve-type-names:false\n"
        ));
    }

    #[test]
    fn ignores_trailing_garbage_on_directive_line() {
        // Anything after `false` on the same line invalidates the match
        // (upstream's `$` is end-of-line in default regex mode).
        assert!(!is_type_name_resolution_disabled(
            "# resolve-type-names: false junk\n"
        ));
    }
}
