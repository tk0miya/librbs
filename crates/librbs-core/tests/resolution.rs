//! M3b integration tests: resolution driver.
//!
//! Each test builds a small `Environment` from an inline RBS source and
//! exercises one slice of the M3b acceptance criteria:
//!
//! * the per-`DeclRef` `Vec<ResolvedRef>` slice contains the expected
//!   resolution outcomes in pre-order;
//! * `# resolve-type-names: false` short-circuits resolution for that
//!   source;
//! * for every entry, the stored `DeclRef` resolves back to a parsed
//!   node whose name and kind match the entry — the M2 follow-up
//!   "DeclRef indexing consistency between insert and lookup".
//!
//! Canonical-dump fixtures are intentionally absent: the Rust-side
//! `canonical_dump` was deferred to a followup. Compatibility checks
//! happen on the Ruby side from M3c onward.

use std::path::PathBuf;

use librbs_core::SourceTag;
use librbs_core::env::Environment;
use librbs_core::env::entry::{ClassLikeEntry, DeclRef};
use librbs_core::env::insert::insert_rbs_source;
use librbs_core::env::resolution::ResolvedRef;
use librbs_core::resolver::driver::{lookup_decl, resolve};
use librbs_core::source::Source;
use ruby_rbs::node::Node;

fn build_env(rbs_sources: &[&str]) -> Environment {
    let mut env = Environment::new();
    let mut sources: Vec<Source> = Vec::new();
    for (i, src) in rbs_sources.iter().enumerate() {
        let path = PathBuf::from(format!("/tmp/{i}.rbs"));
        let s = Source::new(SourceTag::Dir(path.clone()), path, src.to_string()).unwrap();
        insert_rbs_source(&mut env, i as u32, s.parser.signature()).unwrap();
        sources.push(s);
    }
    env.sources = sources;
    env
}

#[test]
fn resolves_super_class_to_absolute_name() {
    // `class Foo` and `class Bar < Foo` — the `Foo` reference inside the
    // super-class clause resolves to `::Foo`. The resolution side-table
    // therefore holds one slice for `Bar`'s `DeclRef` containing one
    // `Resolved(::Foo)` entry; `Foo`'s own `DeclRef` has no slice
    // because `Foo` has no super-class and no other type-name references.
    let mut env = build_env(&[r#"class Foo end
class Bar < Foo end
"#]);
    let resolution = resolve(&mut env, None);

    assert_eq!(resolution.len(), 1);
    let bar_decl = DeclRef {
        source_index: 0,
        decl_index: 1,
    };
    let slice = resolution
        .get(bar_decl)
        .expect("Bar's super-class reference should be in the resolution");
    assert_eq!(slice.len(), 1);
    let foo_sym = env
        .class_decls
        .keys()
        .find(|k| env.interner.to_string(**k) == "::Foo")
        .copied()
        .unwrap();
    assert_eq!(slice[0], ResolvedRef::Resolved(foo_sym));
}

#[test]
fn unknown_super_class_is_recorded_as_unresolved() {
    let mut env = build_env(&[r#"class Foo < Unknown end"#]);
    let resolution = resolve(&mut env, None);

    let foo_decl = DeclRef {
        source_index: 0,
        decl_index: 0,
    };
    let slice = resolution.get(foo_decl).unwrap();
    assert_eq!(slice.len(), 1);
    match slice[0] {
        ResolvedRef::Unresolved(sym) => {
            assert_eq!(env.interner.to_string(sym), "Unknown");
        }
        other => panic!("expected Unresolved, got {:?}", other),
    }
}

#[test]
fn magic_comment_disables_resolution_for_that_source() {
    // First source carries the magic comment; second does not. Only the
    // second source's references should appear in the resolution table.
    let mut env = build_env(&[
        "# resolve-type-names: false\nclass A end\nclass B < A end\n",
        "class C end\nclass D < C end\n",
    ]);
    let resolution = resolve(&mut env, None);

    assert!(
        resolution.iter().all(|(dr, _)| dr.source_index == 1),
        "no entries should come from source 0; got {:?}",
        resolution.iter().map(|(dr, _)| dr).collect::<Vec<_>>()
    );
    assert_eq!(resolution.len(), 1);
}

#[test]
fn only_filter_resolves_named_decl_only() {
    // Two top-level classes that each reference an absolute name. With
    // `only` set to `{::Bar}`, only `Bar`'s super-class reference should
    // appear in the resolution table — `Foo`'s body is skipped.
    let mut env = build_env(&[r#"class Base end
class Foo < Base end
class Bar < Base end
"#]);

    let bar_sym = env
        .class_decls
        .keys()
        .find(|k| env.interner.to_string(**k) == "::Bar")
        .copied()
        .unwrap();
    let mut only = rustc_hash::FxHashSet::default();
    only.insert(bar_sym);

    let resolution = resolve(&mut env, Some(&only));

    let base_sym = env
        .class_decls
        .keys()
        .find(|k| env.interner.to_string(**k) == "::Base")
        .copied()
        .unwrap();
    let resolved: Vec<ResolvedRef> = resolution
        .iter()
        .flat_map(|(_, slice)| slice.iter().copied())
        .collect();
    assert_eq!(resolved.len(), 1, "only Bar's super-class should resolve");
    assert_eq!(resolved[0], ResolvedRef::Resolved(base_sym));
}

#[test]
fn declref_round_trip_for_every_entry() {
    // Build a non-trivial environment and verify that for every stored
    // entry the recorded `DeclRef` round-trips back to a node whose
    // name/kind matches the entry.
    let mut env = build_env(&[r#"
class Foo
  class Inner end
end
module Bar end
interface _Each end
type x = Integer
Pi: Integer
$logger: Integer
class A = Foo
"#]);
    // Trigger resolution to populate side-tables (also ensures resolve
    // can run on this fixture).
    let _ = resolve(&mut env, None);

    // ClassLikeEntry: every (context, decl_ref) pair points to a
    // Class/Module node whose simple name matches the entry's last
    // segment.
    for (sym, entry) in env.class_decls.iter() {
        let full = env.interner.to_string(*sym);
        let simple_name = full.rsplit("::").next().unwrap();
        let decls = match entry {
            ClassLikeEntry::Class(c) => &c.context_decls,
            ClassLikeEntry::Module(m) => &m.context_decls,
        };
        for (_ctx, decl_ref) in decls {
            let source = &env.sources[decl_ref.source_index as usize];
            let node = lookup_decl(source, *decl_ref).expect("decl_ref must resolve");
            match (entry, &node) {
                (ClassLikeEntry::Class(_), Node::Class(c)) => {
                    assert_eq!(c.name().name().as_str(), simple_name);
                }
                (ClassLikeEntry::Module(_), Node::Module(m)) => {
                    assert_eq!(m.name().name().as_str(), simple_name);
                }
                (a, b) => panic!("entry/node kind mismatch for {full}: {a:?} vs {b:?}"),
            }
        }
    }

    // Single-decl entries: interface / type_alias / constant / aliases.
    for (sym, entry) in env.interface_decls.iter() {
        let source = &env.sources[entry.decl.source_index as usize];
        let node = lookup_decl(source, entry.decl).expect("interface decl_ref");
        let full = env.interner.to_string(*sym);
        let simple = full.rsplit("::").next().unwrap();
        match node {
            Node::Interface(i) => assert_eq!(i.name().name().as_str(), simple),
            other => panic!("expected Interface for {full}, got {other:?}"),
        }
    }
    for (sym, entry) in env.type_alias_decls.iter() {
        let source = &env.sources[entry.decl.source_index as usize];
        let node = lookup_decl(source, entry.decl).expect("type_alias decl_ref");
        let full = env.interner.to_string(*sym);
        let simple = full.rsplit("::").next().unwrap();
        match node {
            Node::TypeAlias(t) => assert_eq!(t.name().name().as_str(), simple),
            other => panic!("expected TypeAlias for {full}, got {other:?}"),
        }
    }
    for (sym, entry) in env.constant_decls.iter() {
        let source = &env.sources[entry.decl.source_index as usize];
        let node = lookup_decl(source, entry.decl).expect("constant decl_ref");
        let full = env.interner.to_string(*sym);
        let simple = full.rsplit("::").next().unwrap();
        match node {
            Node::Constant(c) => assert_eq!(c.name().name().as_str(), simple),
            other => panic!("expected Constant for {full}, got {other:?}"),
        }
    }
    for (sym, entry) in env.global_decls.iter() {
        let source = &env.sources[entry.decl.source_index as usize];
        let node = lookup_decl(source, entry.decl).expect("global decl_ref");
        let _ = sym;
        match node {
            Node::Global(_) => {}
            other => panic!("expected Global, got {other:?}"),
        }
    }
    for (sym, entry) in env.class_alias_decls.iter() {
        use librbs_core::env::entry::ClassAliasLikeEntry;
        let decl_ref = match entry {
            ClassAliasLikeEntry::Class(c) => c.decl,
            ClassAliasLikeEntry::Module(m) => m.decl,
        };
        let source = &env.sources[decl_ref.source_index as usize];
        let node = lookup_decl(source, decl_ref).expect("alias decl_ref");
        let full = env.interner.to_string(*sym);
        let simple = full.rsplit("::").next().unwrap();
        match node {
            Node::ClassAlias(a) => assert_eq!(a.new_name().name().as_str(), simple),
            Node::ModuleAlias(a) => assert_eq!(a.new_name().name().as_str(), simple),
            other => panic!("expected ClassAlias/ModuleAlias for {full}, got {other:?}"),
        }
    }
}
