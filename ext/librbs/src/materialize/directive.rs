//! M3k: build `RBS::AST::Directives::*` instances from the AST nodes
//! exposed by `signature().directives()`.
//!
//! Two families:
//!
//! - `Use` directives (with `SingleClause` / `WildcardClause` children)
//!   are produced by the C parser and accessible via the AST walk.
//! - `ResolveTypeNames` is parsed in pure Ruby (`parser_aux.rb#magic_comment`)
//!   and prepended to the directive list. The C parser does not expose
//!   it, so we detect the magic comment ourselves and synthesize the
//!   directive object directly from the buffer content.
//!
//! Directives are not affected by `resolve_type_names` — their
//! `type_name` / `namespace` are absolute by parser invariant — so the
//! resolution cursor is not consulted here.

use magnus::{Error, RArray, Value, kwargs, prelude::*, value::ReprValue};

use librbs_core::Source;
use ruby_rbs::node::{
    NamespaceNode, Node, TypeNameNode, UseNode, UseSingleClauseNode, UseWildcardClauseNode,
};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{add_optional_child, add_required_child, make_location};

/// Build the `RArray` that becomes `Source::RBS#directives`.
///
/// Order matches upstream:
/// 1. A single `ResolveTypeNames` directive iff the source's first line
///    is a `# resolve-type-names: true|false` magic comment (mirrors
///    `Parser.parse_signature`'s `dirs.unshift(resolved)`).
/// 2. Each `UseNode` from the C-parser AST walk in source order.
pub fn build_directives(ctx: &mut MaterializeCtx<'_>, src: &Source) -> Result<RArray, Error> {
    let arr = ctx.ruby.ary_new();

    if let Some(value) = magic_resolve_type_names(&src.buffer.content) {
        let directive = build_resolve_type_names(ctx, &src.buffer.content, value)?;
        arr.push(directive)?;
    }

    for dir in src.parser.signature().directives().iter() {
        if let Node::Use(u) = &dir {
            arr.push(build_use(ctx, &u)?)?;
        }
    }

    Ok(arr)
}

fn build_use(ctx: &mut MaterializeCtx<'_>, node: &UseNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "keyword", &node.keyword_location())?;

    let clauses = ctx.ruby.ary_new();
    for clause in node.clauses().iter() {
        match clause {
            Node::UseSingleClause(c) => clauses.push(build_use_single_clause(ctx, &c)?)?,
            Node::UseWildcardClause(c) => clauses.push(build_use_wildcard_clause(ctx, &c)?)?,
            _ => {}
        }
    }

    Ok(ctx
        .classes
        .directives_use
        .new_instance((kwargs!(
            "clauses" => clauses,
            "location" => loc
        ),))?
        .as_value())
}

fn build_use_single_clause(
    ctx: &mut MaterializeCtx<'_>,
    node: &UseSingleClauseNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "type_name", &node.type_name_location())?;
    add_optional_child(ctx, loc, "keyword", node.keyword_location().as_ref())?;
    add_optional_child(ctx, loc, "new_name", node.new_name_location().as_ref())?;

    let type_name_node = node.type_name();
    let type_name = build_type_name_from_ast(ctx, &type_name_node)?;
    let new_name = match node.new_name() {
        Some(sym) => ctx.ruby.to_symbol(sym.as_str()).as_value(),
        None => ctx.ruby.qnil().as_value(),
    };

    Ok(ctx
        .classes
        .directives_use_single_clause
        .new_instance((kwargs!(
            "type_name" => type_name,
            "new_name" => new_name,
            "location" => loc
        ),))?
        .as_value())
}

fn build_use_wildcard_clause(
    ctx: &mut MaterializeCtx<'_>,
    node: &UseWildcardClauseNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &node.location())?;
    add_required_child(ctx, loc, "namespace", &node.namespace_location())?;
    add_required_child(ctx, loc, "star", &node.star_location())?;

    let namespace_node = node.namespace();
    let namespace = build_namespace_from_ast(ctx, &namespace_node)?;

    Ok(ctx
        .classes
        .directives_use_wildcard_clause
        .new_instance((kwargs!(
            "namespace" => namespace,
            "location" => loc
        ),))?
        .as_value())
}

/// Build `RBS::AST::Directives::ResolveTypeNames` for a known magic
/// comment. The location's char offsets are derived from the buffer
/// content directly — the C parser does not expose them.
fn build_resolve_type_names(
    ctx: &mut MaterializeCtx<'_>,
    content: &str,
    value: bool,
) -> Result<Value, Error> {
    let buffer = ctx.buffer()?;
    let layout = magic_comment_layout(content)
        .expect("magic_comment_layout must succeed when magic_resolve_type_names did");

    let kw_start = layout.kw_start as i32;
    let kw_end = layout.kw_end as i32;
    let colon_start = layout.colon_start as i32;
    let colon_end = layout.colon_end as i32;
    let value_start = layout.value_start as i32;
    let value_end = layout.value_end as i32;

    let loc = ctx
        .classes
        .location
        .new_instance((buffer, kw_start, value_end))?
        .as_value();
    let kw_range = ctx.ruby.range_new(kw_start, kw_end, false)?;
    let _: Value = loc.funcall("add_required_child", (ctx.ruby.to_symbol("keyword"), kw_range))?;
    let colon_range = ctx.ruby.range_new(colon_start, colon_end, false)?;
    let _: Value = loc.funcall(
        "add_required_child",
        (ctx.ruby.to_symbol("colon"), colon_range),
    )?;
    let value_range = ctx.ruby.range_new(value_start, value_end, false)?;
    let _: Value = loc.funcall(
        "add_required_child",
        (ctx.ruby.to_symbol("value"), value_range),
    )?;

    Ok(ctx
        .classes
        .directives_resolve_type_names
        .new_instance((kwargs!(
            "value" => value,
            "location" => loc
        ),))?
        .as_value())
}

struct MagicCommentLayout {
    kw_start: usize,
    kw_end: usize,
    colon_start: usize,
    colon_end: usize,
    value_start: usize,
    value_end: usize,
}

/// Return the boolean payload of a `# resolve-type-names: ...` magic
/// comment if the first line of `content` matches the same regex
/// `Parser.magic_comment` uses upstream. Returns `None` otherwise.
///
/// Matches both `true` and `false` so the directive object reflects
/// the source verbatim. (The resolver-side helper
/// `is_type_name_resolution_disabled` only short-circuits on `false`,
/// since `true` and "no directive" produce identical resolution.)
fn magic_resolve_type_names(content: &str) -> Option<bool> {
    let layout = magic_comment_layout(content)?;
    match &content[layout.value_start..layout.value_end] {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Locate the char offsets of `keyword`, `colon`, and `value` within the
/// magic comment on the first line of `content`. Mirrors the regex
/// `\A#\s*(?<keyword>resolve-type-names)\s*(?<colon>:)\s+(?<value>true|false)$`
/// in `Parser.magic_comment`. Returns `None` if the first line does not
/// match.
///
/// Char-offset based: the magic-comment regex content is pure ASCII so
/// byte and char offsets coincide on the matched span itself, but we
/// expose char offsets to match `RBSLocationRange::start_char`.
fn magic_comment_layout(content: &str) -> Option<MagicCommentLayout> {
    let first_line = content.lines().next()?;
    if !first_line.starts_with('#') {
        return None;
    }

    let mut idx: usize = 1; // past `#`
    let bytes = first_line.as_bytes();
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    let kw_start = idx;
    let kw = "resolve-type-names";
    if !first_line.get(kw_start..)?.starts_with(kw) {
        return None;
    }
    let kw_end = kw_start + kw.len();
    idx = kw_end;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    if first_line.as_bytes().get(idx)? != &b':' {
        return None;
    }
    let colon_start = idx;
    let colon_end = colon_start + 1;
    idx = colon_end;
    let ws_start = idx;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    if idx == ws_start {
        return None; // require at least one whitespace
    }
    let value_start = idx;
    let rest = first_line.get(value_start..)?;
    let value_end = if rest == "true" || rest == "false" {
        value_start + rest.len()
    } else {
        return None;
    };

    Some(MagicCommentLayout {
        kw_start,
        kw_end,
        colon_start,
        colon_end,
        value_start,
        value_end,
    })
}

/// Build `RBS::TypeName` from a parsed `TypeNameNode` directly, without
/// consulting the interner. Used for directive payloads where the
/// type-name might not have been interned yet (the env may not have
/// been resolved) and where resolution does not apply anyway.
fn build_type_name_from_ast(
    ctx: &MaterializeCtx<'_>,
    node: &TypeNameNode<'_>,
) -> Result<Value, Error> {
    let ns_node = node.namespace();
    let namespace = build_namespace_from_ast(ctx, &ns_node)?;
    let name = ctx.ruby.to_symbol(node.name().as_str());
    Ok(ctx
        .classes
        .type_name
        .new_instance((kwargs!("namespace" => namespace, "name" => name),))?
        .as_value())
}

/// Build `RBS::Namespace` from a parsed `NamespaceNode`.
fn build_namespace_from_ast(
    ctx: &MaterializeCtx<'_>,
    node: &NamespaceNode<'_>,
) -> Result<Value, Error> {
    let absolute = node.absolute();
    let path = ctx.ruby.ary_new();
    for seg in node.path().iter() {
        if let Node::Symbol(sym) = seg {
            path.push(ctx.ruby.to_symbol(sym.as_str()))?;
        }
    }
    Ok(ctx
        .classes
        .namespace
        .new_instance((kwargs!("path" => path, "absolute" => absolute),))?
        .as_value())
}

