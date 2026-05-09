//! M3k: build `RBS::AST::Directives::*` instances for a source.
//!
//! `Use` directives come from the C parser via `signature().directives()`.
//! `ResolveTypeNames` is a magic comment parsed in Ruby upstream
//! (`vendor/rbs/lib/rbs/parser_aux.rb#magic_comment`) and prepended to
//! the directives array; the C parser does not expose it. We mirror that
//! behavior by scanning the first line of the buffer ourselves.
//!
//! Directive type-names are *not* affected by `resolve_type_names` —
//! `Use` clauses are absolute by parser invariant, and `ResolveTypeNames`
//! carries a boolean. We materialize directly from the AST without
//! consulting the resolution side-table.

use magnus::{Error, RArray, Value, kwargs, prelude::*, value::ReprValue};

use ruby_rbs::node::{Node, NamespaceNode, TypeNameNode, UseNode, UseSingleClauseNode, UseWildcardClauseNode};

use librbs_core::Source;

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{add_optional_child, add_required_child, make_location};

/// Build the per-source `directives` array. Output order is:
///
/// 1. Synthetic `ResolveTypeNames` directive when the source begins
///    with the `# resolve-type-names: true|false` magic comment
///    (mirrors `parser_aux.rb#parse_signature` prepending it to
///    `dirs`).
/// 2. Every `Use` directive emitted by the C parser, in source order.
pub fn build_directives(ctx: &mut MaterializeCtx<'_>, src: &Source) -> Result<RArray, Error> {
    let arr = ctx.ruby.ary_new();
    if let Some(rt) = build_resolve_type_names(ctx, &src.buffer.content)? {
        arr.push(rt)?;
    }
    for d in src.parser.signature().directives().iter() {
        if let Node::Use(u) = &d {
            arr.push(build_use(ctx, &u)?)?;
        }
    }
    Ok(arr)
}

// ----- Use -----

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

    let type_name = build_type_name_from_node(ctx, &node.type_name())?;
    let new_name: Value = match node.new_name() {
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

    let namespace = build_namespace_from_node(ctx, &node.namespace())?;

    Ok(ctx
        .classes
        .directives_use_wildcard_clause
        .new_instance((kwargs!(
            "namespace" => namespace,
            "location" => loc
        ),))?
        .as_value())
}

// ----- ResolveTypeNames -----

/// Detect a `# resolve-type-names: true|false` magic comment on the
/// first line and build the corresponding `ResolveTypeNames` directive.
/// Returns `Ok(None)` when no magic comment is present.
///
/// Mirrors the regex in `vendor/rbs/lib/rbs/parser_aux.rb#magic_comment`:
///
/// ```text
/// /\A#\s*(?<keyword>resolve-type-names)\s*(?<colon>:)\s+(?<value>true|false)$/
/// ```
///
/// The location's start/end and required `keyword` / `colon` / `value`
/// children match upstream's offsets exactly.
fn build_resolve_type_names(
    ctx: &mut MaterializeCtx<'_>,
    content: &str,
) -> Result<Option<Value>, Error> {
    let Some(parsed) = parse_resolve_type_names(content) else {
        return Ok(None);
    };

    let buffer = ctx.buffer()?;
    let loc: Value = ctx
        .classes
        .location
        .new_instance((buffer, parsed.start as i64, parsed.end as i64))?
        .as_value();

    let kw_sym = ctx.ruby.to_symbol("keyword");
    let kw_range = ctx
        .ruby
        .range_new(parsed.keyword.0 as i64, parsed.keyword.1 as i64, false)?;
    let _: Value = loc.funcall("add_required_child", (kw_sym, kw_range))?;

    let colon_sym = ctx.ruby.to_symbol("colon");
    let colon_range = ctx
        .ruby
        .range_new(parsed.colon.0 as i64, parsed.colon.1 as i64, false)?;
    let _: Value = loc.funcall("add_required_child", (colon_sym, colon_range))?;

    let value_sym = ctx.ruby.to_symbol("value");
    let value_range = ctx
        .ruby
        .range_new(parsed.value.0 as i64, parsed.value.1 as i64, false)?;
    let _: Value = loc.funcall("add_required_child", (value_sym, value_range))?;

    let value_v: Value = if parsed.value_bool {
        ctx.ruby.qtrue().as_value()
    } else {
        ctx.ruby.qfalse().as_value()
    };

    Ok(Some(
        ctx.classes
            .directives_resolve_type_names
            .new_instance((kwargs!(
                "value" => value_v,
                "location" => loc
            ),))?
            .as_value(),
    ))
}

struct MagicComment {
    /// Inclusive..exclusive char positions for the directive's overall
    /// location (from `keyword` start to `value` end), matching upstream.
    start: usize,
    end: usize,
    keyword: (usize, usize),
    colon: (usize, usize),
    value: (usize, usize),
    value_bool: bool,
}

/// Match `\A#\s*resolve-type-names\s*:\s+(true|false)$` against the
/// very first line of `content` and return the offsets of each capture.
/// All matched bytes are ASCII, so byte offsets coincide with character
/// offsets — the returned positions are valid as `RBS::Location` inputs
/// without any byte→char conversion.
fn parse_resolve_type_names(content: &str) -> Option<MagicComment> {
    let first_line = content.lines().next()?;
    // The regex is anchored on `\A`, so first_line starts at byte 0 of
    // `content`. Char positions here are byte positions.
    let bytes = first_line.as_bytes();
    let mut i: usize = 0;
    // \A#
    if bytes.first().copied()? != b'#' {
        return None;
    }
    i += 1;
    // \s*
    i = skip_ws(bytes, i);
    // resolve-type-names
    let kw = b"resolve-type-names";
    if i + kw.len() > bytes.len() || &bytes[i..i + kw.len()] != kw {
        return None;
    }
    let kw_start = i;
    let kw_end = i + kw.len();
    i = kw_end;
    // \s*
    i = skip_ws(bytes, i);
    // :
    if bytes.get(i).copied()? != b':' {
        return None;
    }
    let colon_start = i;
    let colon_end = i + 1;
    i = colon_end;
    // \s+ (at least one)
    let after_ws = skip_ws(bytes, i);
    if after_ws == i {
        return None;
    }
    i = after_ws;
    // (true|false)
    let (value_bool, val_len) = if bytes.get(i..i + 4) == Some(b"true") {
        (true, 4)
    } else if bytes.get(i..i + 5) == Some(b"false") {
        (false, 5)
    } else {
        return None;
    };
    let val_start = i;
    let val_end = i + val_len;
    // $ — must be at end of line
    if val_end != bytes.len() {
        return None;
    }
    Some(MagicComment {
        start: kw_start,
        end: val_end,
        keyword: (kw_start, kw_end),
        colon: (colon_start, colon_end),
        value: (val_start, val_end),
        value_bool,
    })
}

fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while let Some(&b) = bytes.get(i) {
        if b == b' ' || b == b'\t' {
            i += 1;
        } else {
            break;
        }
    }
    i
}

// ----- Namespace / TypeName helpers (AST → Ruby) -----
//
// Directives' `type_name` / `namespace` fields are not interned by the
// M2 insert pass (insert.rs only walks declaration bodies). Walking the
// AST node directly bypasses the interner entirely.

fn build_namespace_from_node(
    ctx: &MaterializeCtx<'_>,
    node: &NamespaceNode<'_>,
) -> Result<Value, Error> {
    let path = ctx.ruby.ary_new();
    for seg in node.path().iter() {
        if let Node::Symbol(sym) = seg {
            path.push(ctx.ruby.to_symbol(sym.as_str()))?;
        }
    }
    Ok(ctx
        .classes
        .namespace
        .new_instance((kwargs!(
            "path" => path,
            "absolute" => node.absolute()
        ),))?
        .as_value())
}

fn build_type_name_from_node(
    ctx: &MaterializeCtx<'_>,
    node: &TypeNameNode<'_>,
) -> Result<Value, Error> {
    let namespace = build_namespace_from_node(ctx, &node.namespace())?;
    let name = ctx.ruby.to_symbol(node.name().as_str()).as_value();
    Ok(ctx
        .classes
        .type_name
        .new_instance((kwargs!(
            "namespace" => namespace,
            "name" => name
        ),))?
        .as_value())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resolve_type_names_false() {
        let m = parse_resolve_type_names("# resolve-type-names: false\nclass Foo end\n").unwrap();
        assert!(!m.value_bool);
        assert_eq!(m.start, 2);
        assert_eq!(m.end, 27);
        assert_eq!(m.keyword, (2, 20));
        assert_eq!(m.colon, (20, 21));
        assert_eq!(m.value, (22, 27));
    }

    #[test]
    fn parses_resolve_type_names_true() {
        let m = parse_resolve_type_names("# resolve-type-names: true").unwrap();
        assert!(m.value_bool);
        assert_eq!(m.value, (22, 26));
    }

    #[test]
    fn rejects_no_magic_comment() {
        assert!(parse_resolve_type_names("class Foo end\n").is_none());
        assert!(parse_resolve_type_names("# unrelated\n").is_none());
        assert!(parse_resolve_type_names("# resolve-type-names:false\n").is_none()); // \s+ required
        assert!(parse_resolve_type_names("# resolve-type-names: maybe\n").is_none());
        assert!(parse_resolve_type_names("# resolve-type-names: false trailing\n").is_none());
    }
}
