//! Materialise `RBS::AST::Directives::*` Ruby values from a source's
//! parsed AST plus a magic-comment scan.
//!
//! [`materialize_directives`] is the single public entry point: it
//! takes the parsed `# use ...` directive list and the source's
//! buffer content, scans the content head for
//! `# resolve-type-names: true|false`, and returns a Ruby
//! `Array[RBS::AST::Directives::*]` in upstream's
//! `[resolve_type_names?, *use_dirs]` order. `Source::RBS#directives`
//! consumes this array verbatim.
//!
//! The magic-comment regex matches upstream's
//! `vendor/rbs/lib/rbs/parser_aux.rb:46-68` exactly (anchor, leading
//! `#`, optional whitespace, literal `resolve-type-names`, optional
//! whitespace, `:`, **required** whitespace, `true|false`, end of
//! line).

use magnus::{Error, Value, kwargs, prelude::*, value::ReprValue};

use ruby_rbs::node::{Node, NodeList, UseNode, UseSingleClauseNode, UseWildcardClauseNode};

use crate::materialize::MaterializeCtx;
use crate::materialize::location::{
    add_optional_child, add_required_child, alloc_children, make_location,
};
use crate::materialize::type_name::materialize_namespace;

/// Build the Ruby `Array[RBS::AST::Directives::*]` for one source.
///
/// `directives` is the parser-emitted directive list
/// (`source.parser.signature().directives()`); `content` is the same
/// source's buffer content, scanned for the
/// `# resolve-type-names: true|false` magic comment that the C parser
/// does not surface as a directive node.
///
/// Order matches upstream `Source::RBS#directives`:
/// `[resolve_type_names?, *use_dirs]`. Non-`Use` directive nodes
/// inside `directives` are skipped (the parser only ever emits `Use`
/// here, but the loop is defensive).
pub fn materialize_directives(
    ctx: &mut MaterializeCtx<'_>,
    directives: NodeList<'_>,
    content: &str,
) -> Result<Value, Error> {
    let arr = ctx.ruby.ary_new_capa(directives.len() + 1);
    if let Some(magic) = materialize_magic_comment(ctx, content)? {
        arr.push(magic)?;
    }
    for dir in directives.iter() {
        if let Node::Use(u) = &dir {
            arr.push(materialize_use(ctx, u)?)?;
        }
    }
    Ok(arr.as_value())
}

fn materialize_use(ctx: &mut MaterializeCtx<'_>, u: &UseNode<'_>) -> Result<Value, Error> {
    let loc = make_location(ctx, &u.location())?;
    add_required_child(ctx, loc, "keyword", u.keyword_location())?;

    let clauses_list = u.clauses();
    let clauses = ctx.ruby.ary_new_capa(clauses_list.len());
    for clause in clauses_list.iter() {
        match clause {
            Node::UseSingleClause(c) => clauses.push(materialize_use_single_clause(ctx, &c)?)?,
            Node::UseWildcardClause(c) => {
                clauses.push(materialize_use_wildcard_clause(ctx, &c)?)?
            }
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

fn materialize_use_single_clause(
    ctx: &mut MaterializeCtx<'_>,
    c: &UseSingleClauseNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &c.location())?;
    alloc_children(ctx, loc, 3);
    add_required_child(ctx, loc, "type_name", c.type_name_location())?;
    add_optional_child(ctx, loc, "keyword", c.keyword_location())?;
    add_optional_child(ctx, loc, "new_name", c.new_name_location())?;

    let type_name = build_directive_type_name(ctx, &c.type_name())?;
    let new_name: Value = match c.new_name() {
        Some(sym) => ctx.symbol_for_str(sym.as_str()),
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

fn materialize_use_wildcard_clause(
    ctx: &mut MaterializeCtx<'_>,
    c: &UseWildcardClauseNode<'_>,
) -> Result<Value, Error> {
    let loc = make_location(ctx, &c.location())?;
    alloc_children(ctx, loc, 2);
    add_required_child(ctx, loc, "namespace", c.namespace_location())?;
    add_required_child(ctx, loc, "star", c.star_location())?;

    let ns_node = c.namespace();
    let namespace = build_namespace_from_node(ctx, &ns_node)?;

    Ok(ctx
        .classes
        .directives_use_wildcard_clause
        .new_instance((kwargs!(
            "namespace" => namespace,
            "location" => loc
        ),))?
        .as_value())
}

/// Build `RBS::TypeName` directly from a parsed `TypeNameNode`. Unlike
/// the type-name-in-decl path this does not consult the interner —
/// use-directive type names are not guaranteed to be pre-interned (the
/// resolver interns them lazily, and unresolved envs never run the
/// resolver), and a use directive's reference shape is preserved
/// verbatim from source either way.
fn build_directive_type_name(
    ctx: &MaterializeCtx<'_>,
    node: &ruby_rbs::node::TypeNameNode<'_>,
) -> Result<Value, Error> {
    let ns_node = node.namespace();
    let namespace = build_namespace_from_node(ctx, &ns_node)?;
    let name_sym = ctx.symbol_for_str(node.name().as_str());
    Ok(ctx
        .classes
        .type_name
        .new_instance((kwargs!("namespace" => namespace, "name" => name_sym),))?
        .as_value())
}

fn build_namespace_from_node(
    ctx: &MaterializeCtx<'_>,
    ns_node: &ruby_rbs::node::NamespaceNode<'_>,
) -> Result<Value, Error> {
    let absolute = ns_node.absolute();
    // Try the interner-cached path first to share `RBS::Namespace`
    // identity with the rest of materialisation when the namespace was
    // already interned. Falls through to a direct build otherwise.
    let path_list = ns_node.path();
    let mut path: Vec<librbs_core::interner::Sym> = Vec::with_capacity(path_list.len());
    let mut interned_ok = true;
    for seg in path_list.iter() {
        if let Node::Symbol(sym) = seg {
            match ctx.interner.symbols().intern(sym.as_str()) {
                Some(s) => path.push(s),
                None => {
                    interned_ok = false;
                    break;
                }
            }
        }
    }
    if interned_ok && let Some(ns_sym) = ctx.interner.namespaces().intern(&path, absolute) {
        return materialize_namespace(ctx, ns_sym, absolute);
    }

    // Fallback: build the path array straight from the AST node.
    let path_array = ctx.ruby.ary_new_capa(path_list.len());
    for seg in path_list.iter() {
        if let Node::Symbol(sym) = seg {
            path_array.push(ctx.symbol_for_str(sym.as_str()))?;
        }
    }
    Ok(ctx
        .classes
        .namespace
        .new_instance((kwargs!("path" => path_array, "absolute" => absolute),))?
        .as_value())
}

/// Detect and materialise a `# resolve-type-names: true|false` magic
/// comment at the very start of `content`. Mirrors upstream
/// `RBS::Parser.magic_comment` (`vendor/rbs/lib/rbs/parser_aux.rb:46-68`)
/// — anchor at start of buffer, optional whitespace, literal
/// `resolve-type-names`, optional whitespace, `:`, **required**
/// whitespace, `true|false`, end of line. Sub-locations match the
/// upstream order: `keyword`, `colon`, `value`.
///
/// `content` must be the buffer content of the source whose
/// `source_index` is currently set on `ctx` — `ctx.buffer()`
/// resolves to that source's `RBS::Buffer`.
fn materialize_magic_comment(
    ctx: &mut MaterializeCtx<'_>,
    content: &str,
) -> Result<Option<Value>, Error> {
    let Some(m) = match_magic_comment(content) else {
        return Ok(None);
    };

    let buffer = ctx.buffer();
    let loc = ctx
        .classes
        .location
        .new_instance((buffer, m.start as i64, m.end as i64))?
        .as_value();
    alloc_children(ctx, loc, 3);

    add_required_child(
        ctx,
        loc,
        "keyword",
        (m.keyword_start as i32, m.keyword_end as i32),
    )?;
    add_required_child(
        ctx,
        loc,
        "colon",
        (m.colon_start as i32, m.colon_end as i32),
    )?;
    add_required_child(
        ctx,
        loc,
        "value",
        (m.value_start as i32, m.value_end as i32),
    )?;

    let value: Value = if m.value_true {
        ctx.ruby.qtrue().as_value()
    } else {
        ctx.ruby.qfalse().as_value()
    };

    let directive = ctx
        .classes
        .directives_resolve_type_names
        .new_instance((kwargs!(
            "value" => value,
            "location" => loc
        ),))?
        .as_value();
    Ok(Some(directive))
}

struct MagicMatch {
    start: usize,
    end: usize,
    keyword_start: usize,
    keyword_end: usize,
    colon_start: usize,
    colon_end: usize,
    value_start: usize,
    value_end: usize,
    value_true: bool,
}

/// Character-offset positions of each match component. Mirrors upstream
/// `parser_aux.magic_comment` regex with anchored `\A`. Returns `None`
/// when the source does not begin with the comment.
fn match_magic_comment(content: &str) -> Option<MagicMatch> {
    // Helper: char index from byte index. Magic-comment positions are
    // exposed to Ruby as `RBS::Location` char offsets (matching
    // `RBSLocationRange::start_char`/`end_char`), so we count chars.
    let char_idx_of = |byte_idx: usize| content[..byte_idx].chars().count();

    // Ruby's `\s` character class: space, tab, LF, CR, vertical tab,
    // form feed. The upstream regex uses `\s`, so match the same set
    // here for parity (`vendor/rbs/lib/rbs/parser_aux.rb:51`).
    const WS: [char; 6] = [' ', '\t', '\n', '\r', '\x0b', '\x0c'];

    // \A#
    let rest = content.strip_prefix('#')?;
    let mut byte = 1;

    // \s*
    let trimmed = rest.trim_start_matches(WS);
    byte += rest.len() - trimmed.len();

    // resolve-type-names
    let after_kw = trimmed.strip_prefix("resolve-type-names")?;
    let keyword_start = byte;
    byte += "resolve-type-names".len();
    let keyword_end = byte;

    // \s*
    let trimmed2 = after_kw.trim_start_matches(WS);
    byte += after_kw.len() - trimmed2.len();

    // :
    let after_colon = trimmed2.strip_prefix(':')?;
    let colon_start = byte;
    byte += 1;
    let colon_end = byte;

    // \s+ — at least one whitespace required.
    let trimmed3 = after_colon.trim_start_matches(WS);
    if trimmed3.len() == after_colon.len() {
        return None;
    }
    byte += after_colon.len() - trimmed3.len();

    // (true|false)$
    let (value_str, value_true) = if trimmed3.starts_with("true") {
        ("true", true)
    } else if trimmed3.starts_with("false") {
        ("false", false)
    } else {
        return None;
    };
    let value_start = byte;
    byte += value_str.len();
    let value_end = byte;

    // Must be end-of-line / end-of-string after the value.
    let after_value = &trimmed3[value_str.len()..];
    if !(after_value.is_empty() || after_value.starts_with('\n') || after_value.starts_with('\r')) {
        return None;
    }

    Some(MagicMatch {
        start: char_idx_of(keyword_start),
        end: char_idx_of(value_end),
        keyword_start: char_idx_of(keyword_start),
        keyword_end: char_idx_of(keyword_end),
        colon_start: char_idx_of(colon_start),
        colon_end: char_idx_of(colon_end),
        value_start: char_idx_of(value_start),
        value_end: char_idx_of(value_end),
        value_true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(s: &str) -> bool {
        match_magic_comment(s).is_some()
    }

    #[test]
    fn matches_true_form() {
        let m = match_magic_comment("# resolve-type-names: true\nclass A end\n").unwrap();
        assert!(m.value_true);
    }

    #[test]
    fn matches_false_form() {
        let m = match_magic_comment("# resolve-type-names: false\n").unwrap();
        assert!(!m.value_true);
    }

    #[test]
    fn rejects_no_space_after_colon() {
        assert!(!matches("# resolve-type-names:false\n"));
    }

    #[test]
    fn rejects_unrelated_comment() {
        assert!(!matches("# something else\n"));
    }

    #[test]
    fn rejects_when_not_at_start() {
        assert!(!matches("\n# resolve-type-names: true\n"));
    }

    #[test]
    fn allows_no_space_before_keyword() {
        let m = match_magic_comment("#resolve-type-names: true\n").unwrap();
        assert!(m.value_true);
    }
}
